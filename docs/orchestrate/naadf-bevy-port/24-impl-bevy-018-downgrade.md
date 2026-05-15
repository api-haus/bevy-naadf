# 24 — Impl log: Bevy 0.19-rc.1 → 0.18.x Downgrade

**Date:** 2026-05-15
**Dispatch:** consolidated (design + self-review + impl + log in one agent run).
**Predecessor:** `23-design-bevy-018-downgrade.md`.
**HEAD at dispatch end:** `4211910` (tree unchanged — see §9).
**Status: ABORTED — `commit-ready: no`. Re-dispatch as multi-step `/delegate`.**

This impl log honestly records that the consolidated-mode dispatch landed the
**design + adversarial self-review** to disk, started the mechanical impl, hit
the wall, and reverted the tree before any partial state could mislead the
orchestrator. The work is **tractable but does not fit one consolidated-mode
dispatch**; the orchestrator should re-dispatch as a multi-step `/delegate`
orchestration using the `23-design-…md` blueprint already on disk.

---

## §1. Bevy version: 0.19-rc.1 → 0.18.1

**Decided** target: `bevy = "=0.18.1"` (latest stable 0.18.x). Same feature set
(`free_camera`, `asset_processor`, `basis-universal`, `dlss`,
`force_disable_dlss`, `webgpu`).

**NOT landed** in this dispatch. Tree-version still `=0.19.0-rc.1`.

---

## §2. Cargo.toml changes (planned, NOT landed)

3 files, 3 version pin changes — all `=0.19.0-rc.1` → `=0.18.1`:

- `crates/bevy_naadf/Cargo.toml` (top-level `bevy` dep) — 1 line.
- `crates/bevy_naadf/Cargo.toml` (native-target `bevy` dep) — 1 line.
- `crates/bevy-instamat/Cargo.toml` (top-level `bevy` dep) — 1 line.

No new deps. `Cargo.lock` would be regenerated.

---

## §3. New deps added / removed

**None.** This dispatch does NOT add `bevy_egui` per the brief
(`do NOT add bevy_egui in this dispatch`).

---

## §4. API-call site changes — the audit

The full 0.19→0.18 API delta is enumerated in `23-design-bevy-018-downgrade.md`
§4. It is reproduced here as a categorised summary so the next dispatch sees
the deltas in one place.

| Category | Sites (est) | 0.19 form | 0.18 form |
|---|---|---|---|
| Render-graph node form (BIG ONE) | 18 nodes | `pub fn node(... RenderContext ...)` in `add_systems(Core3d, ...)` | `Node`-trait impl + `add_render_graph_node` + `add_render_graph_edges` |
| `init_gpu_resource::<X>()` | 2 | `render_app.init_gpu_resource::<X>()` | `render_app.add_systems(RenderStartup, init_gpu_resource_fn::<X>)` with a small helper |
| `Hdr` import | 2 | `bevy::camera::Hdr` | `bevy::render::view::Hdr` |
| `tonemapping` fn import | 1 | `bevy::core_pipeline::tonemapping::tonemapping` + `.before(tonemapping)` | NO equivalent fn — drop import; the constraint moves into `add_render_graph_edges` as an edge to `Node3d::Tonemapping` |
| `Core3dSystems::PostProcess` | 1 | `.in_set(Core3dSystems::PostProcess)` | NO equivalent — drop; the relative ordering moves into `add_render_graph_edges` |
| `core_pipeline::schedule::Core3d` (as `ScheduleLabel`) | 1 | `add_systems(Core3d, ...)` (schedule label) | `bevy::core_pipeline::core_3d::graph::Core3d` (`RenderSubGraph`) |
| `ViewQuery<...>` system-param | ~4 | `view: ViewQuery<(&ViewTarget, &ExtractedView)>` | NO equivalent — split into `ViewNode` (per-view) + plain `Node` (view-agnostic) |
| `RecordDiagnostics::as_deref()` | 16 | `let d = render_context.diagnostic_recorder(); let d = d.as_deref();` | `let d = render_context.diagnostic_recorder();` (returns concrete `impl RecordDiagnostics`, no Option in 0.18) |
| `TextFont.font_size: FontSize::Px(_)` | 2 | `FontSize::Px(14.0)` | `14.0` (raw `f32`) |
| `RenderPassDescriptor.multiview_mask` | 1 | `multiview_mask: None,` | field doesn't exist — remove the line |
| `ExtractedView.target_format` | 2 | `view.target_format` | `view.hdr` → `if view.hdr { ViewTarget::TEXTURE_FORMAT_HDR } else { TextureFormat::bevy_default() }` |
| `AssetSaver::save` signature | 1 | 5-param signature with `_asset_path: AssetPath<'_>` | 4-param signature; `AssetPath` import dropped |
| `SavedAsset<'_, '_, T>` | 1 | 2 lifetimes | `SavedAsset<'_, T>` — 1 lifetime |
| `LoadContext::load_builder()` | 1 | `load_context.load_builder()` | `load_context.loader()` |
| `AssetPath::resolve_embed_str` | 1 | `manifest_path.resolve_embed_str(file)` | `manifest_path.resolve_embed(file)` |

Plus more drift that will surface once the build can run further (e.g. tests
calling `RenderPlugin` / `RenderApp` may have small field-shape changes).

---

## §5. wgpu storage-texture viability — CONFIRMED GO (audit-only)

The Phase-C `Rg32Uint` `read_write` storage-texture binding is wgpu-equivalent
between wgpu 27 (Bevy 0.18.1) and wgpu 29 (Bevy 0.19-rc.1):

```text
wgpu-types-27.0.1/src/lib.rs:3182:    Rg32Uint =>  (s_ro_wo, all_flags),
wgpu-types-29.0.3/src/texture/format.rs:982:    Rg32Uint =>  (s_ro_wo, all_flags),
```

Both versions require the adapter to enable the
adapter-specific-format-features path for `read_write` on `Rg32Uint` — the
same path, both versions. Naga 27 and naga 29 both accept
`StorageAccess::LOAD | STORE` for `Rg32Uint` textures. **The §5 GO/NO-GO check
is GO.** The wgpu layer does not block the downgrade.

---

## §6. Test count: 116 → unknown

**Tests not run.** The build does not pass — see §7.

The design's §9.1 expectation is that all 116 tests will continue to pass
because the test fixtures use `RenderPlugin` / `RenderApp` in a form that is
byte-equivalent between 0.18 and 0.19. **Unverified.**

---

## §7. Gate results

| Gate | Status | Detail |
|---|---|---|
| `cargo build --workspace` | **FAILED** at 34 errors | First-pass error surface (see §8); the audited delta categories §4 each contributed errors as expected. |
| `cargo test -p bevy-naadf --lib` | NOT RUN — blocked by build | — |
| `cargo run --release --bin e2e_render` (baseline) | NOT RUN — blocked by build | — |
| `cargo run --release --bin e2e_render -- --entities` | NOT RUN | — |
| `cargo run --release --bin e2e_render -- --edit-mode` | NOT RUN | — |
| `cargo run --release --bin e2e_render -- --validate-gpu-construction` | NOT RUN | — |

**No baseline luminance verification.** The Dispatch-A baseline was emissive
247.1, solid 242.0, sky 145.9. Not run.

---

## §8. The abort decision — what hit the wall

After landing the design + adversarial self-review + a partial set of
mechanical edits (Cargo manifests, `Hdr` import, `FontSize::Px → f32`,
`multiview_mask` removal, `AssetSaver::save` 4-param signature, `loader()`
+ `resolve_embed`, `as_deref` removal across 4 files, plus a partial rewrite
of `render/graph.rs` to convert 4 of 18 nodes), I confirmed by error-list
inspection that:

1. **The render-graph-node architectural rewrite is irreducibly large.** 18
   functions to convert into `Node`-trait impls (or `ViewNode` for
   `naadf_final_blit_node`), each ~50-100 lines, totaling ~1500 LOC of careful
   structural rewrite. Each rewrite needs cross-checking against the original
   resource-access pattern (some nodes use `Option<Res<X>>` to gate on
   resource existence — translating to `world.get_resource::<X>()`; some
   nodes mutate state via `ResMut`, which translates differently than
   read-only access; some nodes use `ViewQuery` which translates to either
   `ViewNode` or extracting the view from `world`).

2. **The unaudited drift surface is large.** Within the first `cargo build`
   iteration after the audited edits, I hit `multiview_mask`,
   `AssetSaver::save` signature, `SavedAsset` lifetimes, `load_builder`,
   `resolve_embed_str`, `target_format` (vs `hdr`), `as_deref` — only some of
   which the design's §4 audit predicted. After the render-graph-node
   rewrites land, I expect at least another 1-2 iterations of compile-error
   surfacing in: render-graph node tests
   (`construction/bounds_calc/tests.rs`, `construction/world_change.rs`
   tests, `construction/mod.rs` tests), wgpu descriptor field churn that I
   haven't yet hit, and probably a few more `bevy::*` reexport-path changes
   I haven't catalogued. Estimated additional 30-60 minutes of build-fix
   cycles.

3. **Wall-clock estimate from §10 of the design (4-5 hours)** is *just* the
   mechanical rewriting; it does not include the iterative `cargo build`
   compile-discovery cycles (each cycle takes 1-2 minutes wall-clock on this
   25,547-LOC + 7,535-LOC-WGSL workspace). With 5-10 iterations expected to
   surface all drift, the wall-clock cost is closer to **5-7 hours of
   uninterrupted, focused mechanical work**.

4. **Risk to the production state.** Half-done downgrades land *worse*
   states than either start or end: a half-converted render-graph leaves
   the Bevy app build broken at a non-trivial-to-recover state. The hard
   rule in the brief — *"If any gate fails, STOP and report — do NOT keep
   editing past a failure"* — applies here, escalated: the build is the
   prerequisite gate, and it cannot be made to pass in a tractable single
   dispatch.

**Decision:** revert tree to HEAD (`git stash`), preserve the design + this
log on disk, and escalate to the orchestrator for re-dispatch as a
multi-step `/delegate`.

---

## §9. What was NOT done

- No `bevy_egui` (correct — out of scope per brief).
- No behaviour changes (correct).
- No test removals (correct).
- No WGSL shader changes (correct — wgpu/naga validation is byte-equivalent
  between 27 and 29 for the formats the port uses).
- **Build does not pass** — the 18-node render-graph rewrite was not
  completed.
- **Tests not run.**
- **e2e gates not run.**

---

## §10. Tree state at dispatch end

After `git stash`, the working tree is **clean except for**
`docs/orchestrate/naadf-bevy-port/23-design-bevy-018-downgrade.md` (untracked,
the design + self-review) and this file
(`24-impl-bevy-018-downgrade.md`, untracked, the impl log).

Code state = HEAD `4211910`. **No partial code edits remain on disk.** The
git stash captured the partial work; the orchestrator can `git stash drop`
or `git stash list` to manage it as appropriate.

---

## §11. Recommendation to the orchestrator — multi-step re-dispatch

The design at `23-design-bevy-018-downgrade.md` is **comprehensive** — its
§4 audit + §5 GO/NO-GO + §8 self-review covers the mechanical scope. The
orchestrator should re-dispatch this work as a multi-step `/delegate`:

**Suggested workstream decomposition** (parallel-safe via separate
worktrees, per the same pattern Phase C used):

- **Step 1 — Cargo manifest + simple-mechanical (one dispatch).** Apply the
  3-line Cargo pin change + the audited "simple mechanical" deltas:
  - `Hdr` import path (2 sites).
  - `FontSize::Px → f32` (2 sites).
  - `multiview_mask` field removal (1 site).
  - `AssetSaver::save` 4-param signature + drop `AssetPath` import (1 site).
  - `SavedAsset<'_, '_, T>` → `SavedAsset<'_, T>` (1 site).
  - `LoadContext::load_builder` → `loader` (1 site).
  - `AssetPath::resolve_embed_str` → `resolve_embed` (1 site).
  - `as_deref()` removal on `diagnostic_recorder()` (16 sites across 4 files).
  - `ExtractedView::target_format` → `hdr` + format derivation (2 sites).
  - `init_gpu_resource::<X>()` → `add_systems(RenderStartup, init_gpu_resource_fn::<X>)`
    (2 sites) with the helper added in `render/mod.rs`.

  Done bar: `cargo check` surface shrinks to roughly the 18-node
  render-graph-node errors only.

- **Step 2 — Render-graph-node `Node`-trait rewrite (one dispatch).** Convert
  all 18 node-functions to `Node`-trait impls (+ 1 `ViewNode` for the final
  blit). Update `render/mod.rs` to use `add_render_graph_node` +
  `add_render_graph_edges`. The blueprint is the §4.1 sample in the design
  document. Done bar: `cargo build --workspace` passes.

- **Step 3 — Tests + e2e gates (one dispatch).** Run the four gates;
  triage any test regression. Done bar: 116 tests pass, 4 e2e gates green.

- **Step 4 (optional) — Fresh-eyes review** of step 2's render-graph-edge
  order against `WorldRenderBase.cs:205-441` (the design's §9 high-risk-1).

Estimated total wall-clock for the multi-step re-dispatch: same ~5-7 hours,
but split across 3-4 agent dispatches with explicit checkpointing between
them, so a context-budget collapse in one dispatch does not lose
all the work.

---

## §12. Items the implementer (me) escalated to fresh-eyes

Per the consolidated-mode brief, items I did NOT self-certify (carried
forward from `23-design-bevy-018-downgrade.md` §9.3 + the Independent-Review
section):

1. **Render-graph edge order** (`23-design-…md` §4.1) — `delegate-reviewer`
   should walk the `add_render_graph_edges` chain against
   `WorldRenderBase.cs:205-441` line-by-line, *after* step-2 lands.
2. **`naadf_final_blit_node` is `ViewNode` not `Node`** — fresh-eyes should
   confirm the decision (it accesses `ViewTarget`, which is per-view) and
   that no other node has the same property.
3. **wgpu 27 vs wgpu 29 `Rg32Uint` `read_write` adapter-feature parity** —
   the empirical proof is `--validate-gpu-construction`'s bit-equal gate.
   Fresh-eyes can spot-check the design's §5 once the build can run.

---

## §13. Return contract — for the orchestrator

(a) **Commit-ready:** no — aborted-due-to-scope.
(b) **Final Bevy version:** still `=0.19.0-rc.1` on disk (no Cargo changes
    landed; the stash captured the partial pin changes).
(c) **Gate exit codes + luminance:** N/A (build never passed).
(d) **Test count delta:** N/A.
(e) **API-call sites changed (on disk):** 0 (stashed).
(f) **Deviation from brief:** the dispatch returned without all 4 stages on
    disk in the "implementation landed" sense; instead, the design + this
    abort log are on disk. The brief allowed an "aborted-due-to-wgpu-block"
    return — this is the structurally-analogous
    "aborted-due-to-API-architecture-block": the wgpu layer is GO, but the
    0.19→0.18 render-graph API delta is too large to land cleanly in one
    consolidated-mode dispatch. The honest report path the brief endorses
    ("If any gate fails, STOP and report").
(g) **wgpu storage-texture surprises:** none. The §5 audit says GO; this
    holds up under inspection.
(h) **Load-bearing carry-forward for the user:** the design at
    `23-design-bevy-018-downgrade.md` IS the recipe; the next dispatch
    follows it batch-by-batch.
