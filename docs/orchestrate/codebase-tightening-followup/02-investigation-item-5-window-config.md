# Item 5 — window_config.rs → e2e dep-arrow

## Bailing implementor's stated blocker

This item was **not** bailed-out by an implementor — it was surfaced as a
follow-up by the parent D4-final-cleanup dispatch that landed the
`demo_origin_v` inversion. The two surfacings (impl log of commit `2bb03d1`):

`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1460-1468`
(verbatim — verification-of-the-dep-arrow-inversion-itself paragraph):

> `window_config.rs` lines 47, 48, 69, 70, 99, 100, 122, 123 — production
> code reading `crate::e2e::{E2E_WIDTH, E2E_HEIGHT, …}` constants. **These
> are a separate dep-arrow inversion** (different constants, different audit
> lane) that the D7 architect's Side note 6 did not call out. **Out of scope
> for this dispatch** — flagged for the orchestrator below.

`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1489-1495`
(verbatim — Notes block):

> The other production→e2e arrow at `window_config.rs` (`E2E_WIDTH`,
> `E2E_HEIGHT`, `HORIZON_WIDTH`, `E2E_RESIZE_BOOT_WIDTH`, etc.) is a
> separate inversion: production code reads window dimensions named for
> the e2e harness. Resolving it would either rename the constants
> (semantic shift — the constants are *named* after the e2e gates that use
> them) or move them to a `window_dimensions` module.

`docs/orchestrate/codebase-tightening/render-pipeline/04-refactoring.md:1736-1746`
(verbatim — Step 5 carry-over):

> A separate instance of the same anti-pattern this dispatch resolved for
> `demo_origin_v` — production code reads `crate::e2e::{E2E_WIDTH,
> E2E_HEIGHT, HORIZON_WIDTH, …}` constants from the e2e module. … Resolution
> would either rename the constants (`E2E_WIDTH` → `DEFAULT_WINDOW_WIDTH`?)
> or relocate them to a `window_dimensions` module. ~8 LOC of mechanical
> text + a re-export at `e2e/mod.rs` to keep e2e-harness imports resolving.

Note the impl log's *own* framing is unstable — line 1739 calls it "the
same anti-pattern" but line 1490 immediately concedes "the constants are
named after the e2e gates that use them" (i.e. relocation forces a
semantic shift). The follow-up audit
(`docs/orchestrate/codebase-tightening-followup/00-reuse-audit.md:192-193,
278-288`) elevates that hesitation to an explicit premise-flaw hypothesis.

---

## Verification of the claim

`window_config.rs` imports 8 constants across 4 callsites, sourced from 3
e2e submodules. Per-callsite (file:line cited):

### Callsite 1 — `WindowConfig::e2e()` reads `E2E_WIDTH` / `E2E_HEIGHT`

- `crates/bevy_naadf/src/window_config.rs:44-58` — `WindowConfig::e2e()`
  constructor, lines 47-48 read `crate::e2e::E2E_WIDTH` /
  `crate::e2e::E2E_HEIGHT`.
- Source:
  `crates/bevy_naadf/src/e2e/mod.rs:52-58` — `pub const E2E_WIDTH: u32 =
  256;` / `pub const E2E_HEIGHT: u32 = 256;` with docblock:

  > Fixed e2e window resolution — small + fixed so the readback is fast,
  > the GI dispatch is cheap, and every `pixel_count`-sized buffer is
  > identical run-to-run (`e2e-render-test.md` §4.2 / §9). 256² is large
  > enough for stable region gates.

- **Classification: intrinsically e2e-shaped** (legitimate consumer).
  The constant exists solely because the e2e harness needs a small fixed
  framebuffer for fast deterministic readback. Production never wants
  256×256 — production calls `WindowConfig::windowed()` and gets `None`
  (platform default; verified `app_config.rs:36-46`).
- The reader of `E2E_WIDTH` is, by construction, the e2e-window
  constructor. The constant's *meaning* is "the size the e2e harness pins
  the window to."

### Callsite 2 — `WindowConfig::e2e_horizon()` reads `HORIZON_WIDTH` / `HORIZON_HEIGHT`

- `crates/bevy_naadf/src/window_config.rs:60-76` — constructor with a
  docblock that itself names the constants as e2e-coupled (lines 60-65):

  > The e2e window for the horizon-parity gate (2026-05-19). 1280×720 —
  > large enough that the long-distance raymarch covers the full
  > framebuffer (the standard 256×256 e2e window is too small to make
  > horizon-line ray-termination regressions visible), matched to the
  > Playwright spec's `viewport: { width: 1280, height: 720 }` so the
  > cross-target PNGs SSIM-compare without resize.

- Source: `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:76-80`:

  > Horizon-mode window width. Chosen to match a 1280×720 Playwright viewport
  > so cross-target PNGs SSIM-compare without resize.
  > `pub const HORIZON_WIDTH: u32 = 1280;`

- The horizon gate's *own* docstring at
  `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:42-46` echoes:

  > Native runs at [`HORIZON_WIDTH`]×[`HORIZON_HEIGHT`] = 1280×720 (not the
  > default 256×256 e2e window). The Playwright spec must pin the same
  > viewport so the two PNGs SSIM-compare without resize.

- `HORIZON_WIDTH` is also read in-module at
  `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs:151-152` (logging the
  pose).
- **Classification: intrinsically e2e-shaped** (legitimate consumer). The
  constant is *defined* by the WASM Playwright spec's viewport pin — it is
  not a generic window dimension that happens to be reused for an e2e
  gate. Moving it out of `vox_horizon_parity` would orphan the gate from
  its viewport contract.

### Callsite 3 — `WindowConfig::e2e_resize_test()` reads `E2E_RESIZE_BOOT_WIDTH` / `E2E_RESIZE_BOOT_HEIGHT`

- `crates/bevy_naadf/src/window_config.rs:91-112` — constructor, lines
  99-100 read the boot dimensions. The constructor's own docblock at
  lines 92-97 spells out the *e2e-test-coupled* derivation:

  > User spec for the three-step resize test (boot → 1920×1080 →
  > 2000×1000): the *initial* screenshot is taken at 800×600, so the
  > window boots at exactly that size.

- Source: `crates/bevy_naadf/src/e2e/mod.rs:156-161` with docblock:

  > Boot width for the resize-test window (user spec: "start the game in
  > 800×600"). At 60 fps the harness reaches the first screenshot after
  > [`E2E_RESIZE_LAUNCH_SETTLE_FRAMES`] ticks ≈ 5 s post-launch.

- The constant is also read in-module at
  `crates/bevy_naadf/src/e2e/driver.rs:897-898` (the resize-test PASS log).
- **Classification: intrinsically e2e-shaped** (legitimate consumer). The
  resize-test gate IS the consumer, and the boot dimension is one slot in
  the gate's `boot → A → B` size-progression contract (the `E2E_RESIZE_A_*`
  and `E2E_RESIZE_B_*` siblings at `e2e/mod.rs:163-171` stay e2e-only —
  there is no production-side use of "the size the window resizes to next").

### Callsite 4 — `WindowConfig::e2e_small_edit_repro()` reads `SMALL_EDIT_REPRO_WIDTH` / `SMALL_EDIT_REPRO_HEIGHT`

- `crates/bevy_naadf/src/window_config.rs:119-129` — constructor, lines
  122-123 read the dimensions. The constructor's own docblock at lines
  114-118:

  > The e2e window for the `--small-edit-repro` gate (2026-05-17). Runs at
  > the user's screen size (1920×1080) so the bug-or-fix signal matches
  > what the user observes in the live binary. … the user's report
  > specifies this size; we reproduce verbatim.

- Source: `crates/bevy_naadf/src/e2e/small_edit_repro.rs:100-107`:

  > Window resolution — match the user's screen, 1920×1080. The bug is
  > AADF-driven so it shows at any resolution, but the user reported it
  > at 1920×1080 so we reproduce there.
  > `pub const SMALL_EDIT_REPRO_WIDTH: u32 = 1920;`

- **Classification: intrinsically e2e-shaped** (legitimate consumer).
  The constant is the *reproduction's* viewport — fixed at 1920×1080
  because the user's session was at 1920×1080. Production has no claim
  on this number; the value's authority is the user's bug report.

### Caller-side observation

Every method on `WindowConfig` that reads any of these 8 constants is
itself an `e2e_*` constructor (`e2e`, `e2e_horizon`, `e2e_resize_test`,
`e2e_small_edit_repro`). The dispatcher `window_for_e2e_args`
(`window_config.rs:137-147`) returns one of them — and is called from
exactly one place: `crates/bevy_naadf/src/lib.rs:355` inside
`run_e2e_render_with_args`, the e2e binary entry point. Production
(`app_config.rs:43`) uses `WindowConfig::windowed()`, which reads zero
e2e constants.

**The 8 constants are read by e2e-mode-only code paths in `window_config.rs`.**
The "production reads e2e" framing is technically true at the module-
boundary level (`window_config.rs` is not under `e2e/`) but semantically
false at the call-graph level (every reader is an e2e-mode constructor
that only e2e callers ever invoke).

---

## demo_origin_v template applicability

The `demo_origin_v` inversion (commit `2bb03d1`) was *not* shaped like
this case. Verifying the template against the post-commit source:

- Pre-inversion: `demo_origin_v()` *lived* in `e2e/gates.rs` but its body
  read `WORLD_SIZE_IN_CHUNKS` (a production constant) and called
  `(WORLD_SIZE_IN_CHUNKS.x * 16 - small_in_voxels_x) / 2` —
  i.e. it computed a *production-space* coordinate.
- Post-inversion home: `crates/bevy_naadf/src/voxel/grid.rs:83-89`
  defines `pub fn demo_origin_v() -> Vec3 { … }`. The body reads
  `DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS` (defined in the same file at
  `voxel/grid.rs:67`) and `WORLD_SIZE_IN_CHUNKS` (production world-size
  constant). The function is now *next to* the production data it
  consumes.
- Post-inversion shim: `crates/bevy_naadf/src/e2e/gates.rs:23-30` keeps a
  `pub use crate::voxel::grid::demo_origin_v;` re-export so unchanged
  e2e-side callers still resolve.
- Post-inversion production caller: `test_fixture.rs:65` reads
  `crate::voxel::grid::demo_origin_v()` directly; the module docstring
  at `test_fixture.rs:15-23` documents the inversion.

**Why the template fit `demo_origin_v`:** the function was a
small-default-scene → fixed-world coordinate translator. It had a
*production* home (the world data lives in `voxel/grid.rs`); it only
sat in `e2e/gates.rs` historically because the e2e harness was the
first consumer. Moving it to `voxel/grid.rs` aligned the function's
home with the data it reads. The e2e re-export costs 1 LOC.

**Why the template does NOT fit the 8 window-dim constants:**

1. The constants are not *generic* dimensions that the e2e harness
   happened to first consume. Each constant's *value* is set by the
   gate's contract:
   - `E2E_WIDTH=256` is "the size the e2e harness pins for fast
     readback + cheap GI dispatch" (`e2e/mod.rs:52-55`). Production
     has no reason to want 256×256.
   - `HORIZON_WIDTH=1280` is "pinned to the Playwright viewport"
     (`vox_horizon_parity.rs:76-77`). The number lives downstream of a
     WASM-side spec.
   - `E2E_RESIZE_BOOT_WIDTH=800` is one slot in the resize-test's
     `boot → A → B` size progression (`e2e/mod.rs:148-161`). Moving
     just the boot value out and leaving `A` / `B` in e2e breaks the
     contract's locality.
   - `SMALL_EDIT_REPRO_WIDTH=1920` is "the user's reproduction
     screen size" (`small_edit_repro.rs:100-107`). The value's
     authority is the bug-report session, not a production sizing
     decision.

2. There is no analogue of `DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS` in
   production land — the e2e dimensions don't *relocate* to a richer
   home, they would have to be *invented* one. A `window_dimensions.rs`
   module that just hoists the same constants up one level adds an
   indirection without changing the semantic ownership; the values
   would still be set by, and meaningful only inside, the e2e gates.

3. The `demo_origin_v` inversion targeted *one* function with one
   non-e2e production caller (`test_fixture.rs`). Here the production
   "callers" are themselves the four `WindowConfig::e2e_*` constructors
   — which are *only ever invoked* through the e2e entry point
   (`lib.rs:355`). There is no production-mode reader.

The template only matches if (a) the moved entity has natural
non-e2e home and (b) there's a production-mode caller that benefits
from breaking the e2e-module dependency. Neither holds here.

---

## Diagnosis

**Category (c): mixed — but heavily skewed to "legitimate consumer."**

- All 8 constants are **intrinsically e2e-shaped** (per-constant
  verification above).
- The "production" side of the dep arrow is technically `window_config.rs`
  (not under `e2e/`), but its only readers of e2e constants are the four
  `e2e_*` constructors, and those constructors' only caller is the e2e
  entry point (`lib.rs:355` via `window_for_e2e_args`). The module
  boundary is the right grain to enforce reuse separation; this dep arrow
  doesn't actually couple production-mode code to e2e logic.
- The audit's premise-flaw hypothesis
  (`00-reuse-audit.md:278-288`) is **confirmed**. The brief's
  `demo_origin_v`-parallel framing does not fit; the right resolution
  is not a relocation.

The only residual smell is that `window_config.rs` sits at
`crates/bevy_naadf/src/window_config.rs` (top-level, peer of `e2e/`)
rather than under `e2e/`. That is a *module-placement* observation, not a
dep-arrow inversion. The four `e2e_*` constructors logically belong to
the e2e surface; the one `windowed()` constructor is the production
surface. Splitting the file along that line is a separate refactor
question — orthogonal to "is the import direction wrong."

---

## Proposed path forward

**Primary recommendation: (c) accept-as-is + document.**

The four `WindowConfig::e2e_*` constructors are by-design e2e-mode
factories that pin window dimensions defined by the e2e gates they boot.
Each constructor's docblock already names its gate (lines 41-43, 60-65,
78-90, 114-118). The only edit needed is a one-paragraph addition to the
module-level docstring at `window_config.rs:1-10` that documents the
legitimate consumer relationship and explicitly notes the `e2e/`
imports are intentional, not a dep-arrow bug. Suggested text shape:

> Each `WindowConfig::e2e_*` constructor reads its dimensions from the
> e2e gate that boots through it (`crate::e2e::{E2E_WIDTH, …,
> vox_horizon_parity::HORIZON_WIDTH, …}`). These are legitimate
> consumer imports: the constant values are pinned by the gates'
> contracts (Playwright viewport for horizon, user-reported bug-report
> screen size for small-edit-repro, fast-readback choice for the
> 256×256 standard e2e). Relocating them out of `e2e/` would orphan
> them from the gate logic that defines them. The standalone
> `WindowConfig::windowed()` constructor — the only one production
> calls — reads zero e2e constants.

This closes Item 5 cleanly without touching code.

**Optional secondary refactor (orthogonal, not blocking):** split the
e2e-mode constructors out of `window_config.rs` into the e2e module
itself — e.g. a `crate::e2e::window::WindowConfig::e2e*` factory module
that lives next to its source-of-truth constants. The `windowed()`
factory stays in `window_config.rs`. This eliminates the `crate::e2e::…`
imports from `window_config.rs` *structurally* (the e2e-mode code moves
into `e2e/`) rather than by relocating constants. This is a clearer
architectural improvement than the brief's proposal but is a larger
refactor (call-site update in `lib.rs:355`, possibly an
`AppConfig::e2e()` adjustment).

I do not recommend the brief's `demo_origin_v`-shaped relocation
(constants out of `e2e/` into a `window_dimensions.rs`); it adds an
indirection without changing the semantic ownership.

---

## Verification recipe

For category (c) — "non-issue + document":

```sh
# 1) Confirm dep-arrow audit is unchanged (8 lines still present, no new
#    production→e2e imports introduced).
grep -rn "crate::e2e\|use crate::e2e" \
  /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/ \
  | grep -v "^.*/e2e/" \
  | grep -v "//\|///\|//!"
# expect: the 8 known lines in window_config.rs (47, 48, 69, 70, 99, 100,
# 122, 123); no new entries.

# 2) Confirm production-mode binary still uses windowed() (zero e2e
#    constants leak into the production window).
grep -n "WindowConfig::windowed\|WindowConfig::e2e" \
  /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/app_config.rs
# expect: windowed() at line 43; e2e() at line 56. No mixing.

# 3) Build + lib tests (no source change, must stay green).
cd /mnt/archive4/DEV/bevy-naadf
cargo build --workspace
cargo test --workspace --lib

# 4) Each e2e gate that owns one of the 8 constants must still pass
#    (proves the constants' contracts still hold).
cargo run --bin e2e_render -- baseline                  # E2E_WIDTH/HEIGHT
cargo run --bin e2e_render -- --vox-horizon-native      # HORIZON_WIDTH/HEIGHT
cargo run --bin e2e_render -- --resize-test             # E2E_RESIZE_BOOT_WIDTH/HEIGHT
cargo run --bin e2e_render -- --small-edit-repro        # SMALL_EDIT_REPRO_WIDTH/HEIGHT
```

If the orchestrator instead picks the **optional secondary refactor**
(move e2e-mode constructors into `e2e/` module), the verification adds
the canonical dep-arrow grep showing `window_config.rs` lines drop out
+ the same 4 e2e gates above must stay green:

```sh
grep -rn "crate::e2e\|use crate::e2e" \
  /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/ \
  | grep -v "^.*/e2e/" \
  | grep -v "//\|///\|//!"
# expect: empty (or only the rustdoc-link false positives the D7 audit
# already classified harmless at 04-refactoring.md:1462-1463).
```

No non-deterministic gates are involved — all four affected gates are
deterministic (no `--oasis-edit-visual`-style ≥3× rule needed).

---

## Side notes / observations / complaints

- **The brief's premise is wrong, and the impl log that surfaced this
  item already half-knew it.** Lines 1489-1495 of
  `04-refactoring.md` explicitly note "the constants are *named* after
  the e2e gates that use them" and "moving them out of `e2e/` is
  semantically wrong" (paraphrased from the horizon-specific concession
  at audit `00-reuse-audit.md:192`). The orchestrator and the audit
  both flagged the doubt; the brief still framed the item as a real
  inversion. **Confirmed: close as non-issue.**

- **The strongest evidence is the call-graph.** Every reader of the 8
  constants — both inside `window_config.rs` and at the
  `WindowConfig::e2e_*` constructor outputs — is e2e-only at boot. The
  one place a non-e2e reader could appear is `app_config.rs:43`
  (`windowed()`), which reads `resolution: None` (no e2e constant). The
  module-boundary "production reads e2e" framing is a false positive of
  the file-tree audit grep.

- **There IS a real but separate observation:** `window_config.rs` is
  schizophrenic — half of it is e2e-mode factories, half is the
  production factory. Splitting it (or moving the four `e2e_*`
  constructors into `crate::e2e::window`) would eliminate the
  cross-module imports *structurally*. This is a cleaner refactor than
  the brief's proposal, but it's orthogonal to "dep-arrow inversion"
  and should be raised as its own micro-refactor item if anyone wants
  it. Pure cosmetics — does not improve correctness, semantics, or
  reuse.

- **The `demo_origin_v` template is genuinely narrow.** Its applicability
  test is: "does the entity have a natural non-e2e home, AND does a
  production-mode (not just non-`e2e/`) caller benefit from breaking the
  e2e dependency?" For `demo_origin_v` both held (`voxel/grid.rs` is the
  home; `test_fixture.rs::spawn_phase_c_test_entity` is a Startup system
  that runs under production mode too, when `spawn_test_entity = true`).
  Future "this looks like a `demo_origin_v`-shaped inversion" claims
  should run that two-pronged test before assuming the template
  transfers — the file-tree grep is a necessary but very far from
  sufficient signal.

- **Sub-agent compliance:** investigation was strictly read-only as
  briefed; no source-code edits made (only this `.md` written under
  `docs/orchestrate/codebase-tightening-followup/`). Build not run
  (none needed for a read-only diagnosis).

- **Audit corroboration:** every audit citation reproduced under Read
  exactly as quoted (`00-reuse-audit.md:163-193, 278-288`; impl log
  `:1460-1495, 1736-1746`; `voxel/grid.rs:66-89`; `e2e/gates.rs:23-30`;
  `test_fixture.rs:11-22, 61-66`). No source claims required correction.

---

## Closing note (2026-05-21)

Module docstring at `crates/bevy_naadf/src/window_config.rs:1-27` updated
to document the legitimate consumer relationship — closes item 5.
