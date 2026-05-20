# D8 — asset-pipeline exploration

**Author**: asset-pipeline explorer (Phase-1, codebase-tightening orchestration).
**Date**: 2026-05-20.
**Scope**: `crates/bevy_naadf/src/texture_array/**`, `src/baked_material.rs`,
`src/material_set/mod.rs`, `src/bin/bake.rs`. Verified live-vs-dead per
caller audit (sources, e2e, Cargo, justfile, asset tree).
**Verdict at a glance**: the **entire D8 surface is dead-or-orphaned in the
production renderer**. The `bake` binary is live as a justfile recipe
(`just bake-texarrays`), but nothing under `src/` consumes its outputs.
Per the user directive **"everything else can go"**, D8 is a candidate for
near-total deletion — modulo one explicit user decision the architect must
escalate (keep the InstaMAT-pattern scaffolding for future PBR work, or
delete the whole D8 tree end-to-end).

---

## Findings

### Live-vs-dead audit table

| file | LOC | status | evidence |
|---|---|---|---|
| `src/bin/bake.rs` | 96 | **live (binary, off the hot path)** | `justfile:36-37` runs `cargo run -p bevy-naadf --bin bake --no-default-features --release`; the binary's own runtime is the only thing that ever flips `AssetMode::Processed`. |
| `src/texture_array/mod.rs` | 133 | **dead in production, live in bake** | `lib.rs:763` adds `TextureArrayPlugin` to the runtime app, but the loader it registers has no in-tree consumer (no `.load::<Image>("…texarray.ron")` call anywhere). `bin/bake.rs:84` adds the same plugin so the `AssetProcessor` can run the `LoadTransformAndSave` pipeline — that path is the only live use. |
| `src/texture_array/def.rs` | 127 | **dead in production, live in bake** | Consumed only by `loader.rs::TextureArrayLoader::load` and (transitively) by the bake binary's processor. The schema unit tests (in `loader.rs` + `saver.rs`) keep it covered. |
| `src/texture_array/loader.rs` | 331 | **dead in production, live in bake + tests** | `bake_texture_array` is exercised by 4 unit tests (`loader.rs:223-330`) and by the bake binary's processor. Nothing in production calls it. |
| `src/texture_array/saver.rs` | 194 | **live only in bake** | `compress_array_to_basis` is exercised by 2 unit tests + the bake binary's `LoadTransformAndSave`. Compiled out of wasm32 entirely (`mod.rs:89-91`, `Cargo.toml:148-153`). |
| `src/baked_material.rs` | 225 | **plugin registered, loader never invoked** | `lib.rs:758` adds `BakedMaterialPlugin`. Zero in-tree consumers: `grep -rn "load.*StandardMaterial\|load.*material\.ron" src/` returns 0 production call sites and zero `material.ron` files exist anywhere on disk (`find /mnt/archive4/DEV/bevy-naadf -name "material.ron"` → 0 hits). The plugin's own docstring at lines 213-217 admits as much: *"infrastructure only … nothing in the scene is spawned or queried here. Wiring a baked material into a renderer is the consumer's job."* |
| `src/material_set/mod.rs` | 60 | **orphan — not even registered** | `MaterialSetPlugin` is **not** added in `lib.rs:737-764`'s plugin block. It tries to load `materials/{diffuse,normal,mrh,emissive}.texarray.ron` (lines 53-56) but those four files **do not exist** under `src/assets/` (the only `.texarray.ron` on disk is the demo `assets/textures/sample.texarray.ron`). The whole file is investigation residue from `docs/orchestrate/pbr-raymarching/02-design.md` § C — a paused PBR-raymarching design that never landed runtime code. |
| `src/assets/textures/sample.texarray.ron` + `sample_color.png{,.meta}` + `sample_height.png{,.meta}` | n/a (assets) | **demo only, no loader** | The sample asset's own header (line 11) just says *"load it like any image"* — but nothing in the crate loads it. Pure documentation-by-example. |

### Cross-evidence (greps)

- `grep -rn "TextureArrayPlugin\|BakedMaterialPlugin\|MaterialSetPlugin\|MaterialRonLoader\|MaterialSet\|TextureArrayLoader\|baked_material\|texture_array\|material_set" src/`
  outside the D8 modules themselves only matches:
  - `lib.rs:15` (`pub mod baked_material;`), `:23` (`pub mod texture_array;`),
    `:758` (`BakedMaterialPlugin`), `:763` (`TextureArrayPlugin`).
  - **No** `material_set` import anywhere. The module is unreachable from
    `lib.rs` — its `MaterialSetPlugin` is never added to an `App`.
- `grep -rn "asset_server.load" src/` (~30 hits) — every match is either
  a shader load (`render/construction/**`, `render/pipelines.rs`) or a
  vox-related path. **Zero** loads of any `.texarray.ron` or `material.ron`.
- `find ... -name "material.ron"` → 0 results.
- `find ... -name "*.texarray.ron"` → only `src/assets/textures/sample.texarray.ron`
  (the demo file the modules' rustdocs reference) and its mirror in
  `dist/` (build artifact). The four `materials/{diffuse,normal,mrh,emissive}.texarray.ron`
  files `material_set/mod.rs` tries to load **are absent on disk**.
- `grep -rn "MaterialRonLoader\|BakedMaterial" e2e/` → 0 hits.
- `grep -rn "bake\|texarray" e2e/` → 0 hits in e2e tests.
- `justfile:36-37` — `bake-texarrays` recipe — confirmed live entry point
  per `instamat-bake-to-disk` user memory.

### Bug spotted in the dead path (extension collision)

`MaterialRonLoader::extensions()` returns `&["ron"]` (`baked_material.rs:204`)
— i.e. it claims **every `.ron` file** in the asset tree. `TextureArrayLoader::extensions()`
returns `&["texarray.ron"]` (`loader.rs:120`). Bevy's `AssetServer` dispatches
loaders by the longest matching extension; `"texarray.ron"` wins over
`"ron"` for `*.texarray.ron`. But this is a latent footgun: if anyone ever
adds a non-material `.ron` asset (a settings file, a level descriptor, …)
the `MaterialRonLoader` will be invoked, panic on the RON-parse
(`MaterialRonError::Ron(...)`), and the load fails with a confusing
"failed to deserialize material.ron" message even though the consumer
never asked for a `StandardMaterial`. The fix is one line — register
`&["material.ron"]` — but the broader question is whether the loader
should exist at all (see Findings 1 / 4).

---

## Deletion proposal

The architect chooses between **two options**. Both are within the user's
*"everything else can go"* mandate; the difference is whether the InstaMAT
scaffolding survives as the documented template for future PBR work.

### Option A — full nuke (recommended, default)

Delete the entire D8 tree. Rationale: no production consumer, no e2e gate,
no test outside the modules themselves. The InstaMAT-pattern scaffolding
is documented in `docs/orchestrate/pbr-raymarching/01-context.md` and
`02-design.md` — when PBR raymarching resumes, the design doc has the full
recipe and the code can be rebuilt in ~1 200 LOC (which is exactly its
current size). Code in `src/` is for code that runs; templates live in
docs.

**Removed**:

| path | LOC |
|---|---|
| `src/baked_material.rs` | 225 |
| `src/material_set/mod.rs` | 60 |
| `src/texture_array/{mod,def,loader,saver}.rs` | 785 |
| `src/bin/bake.rs` | 96 |
| `src/assets/textures/sample.texarray.ron` | 33 |
| `src/assets/textures/sample_color.png{,.meta}` | n/a |
| `src/assets/textures/sample_height.png{,.meta}` | n/a |
| **D8 total** | **1 199 LOC** + 4 asset files |

**Cargo.toml** removals (and the explanatory comments that go with them):

- `[[bin]] name = "bake"` block (lines 30-32).
- `default-run = "bevy-naadf"` line stays — but the `(`bevy-naadf`, `e2e_render`, `bake`)`
  text in the header comment becomes `(`bevy-naadf`, `e2e_render`)`.
- `image = { ... features = ["png", "jpeg"] }` (lines 65-71) loses its
  texture-array justification — *but* `e2e/framebuffer.rs` still uses
  `image` for PNG encoding (per the existing comment "PNG encoding for the
  e2e harness's persistent screenshot-to-disk"), so the dep stays. The
  `jpeg` feature may now be droppable — architect verifies whether any
  non-D8 site decodes JPEG.
- `ron = "0.12"` (lines 89-92): direct dep was added for `.texarray.ron` /
  `material.ron`. Bevy still pulls `ron` transitively, so dropping the
  direct dep just removes the explanatory comment.
- `serde = { ..., features = ["derive"] }` (lines 93-97): the audit says
  this was added "for `.texarray.ron`". Verify whether other code derives
  `Serialize`/`Deserialize` (the diagnostics device-snapshot module does —
  `diagnostics.rs:248`, `serde_json` is in use), so this dep almost
  certainly stays. Architect verifies.
- `thiserror = "2"`: justified by `texture_array/{loader,saver}.rs` error
  enums per the comment. Verify whether anything else uses `thiserror`
  (`grep -rn "thiserror" src/` outside D8 — if 0, the dep is droppable).
- `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` block (lines
  148-153): the native-only `bevy` extra features `asset_processor` +
  `basis-universal` and the `basis-universal = "0.3"` direct dep are
  **only** for D8. Deleting D8 deletes this whole block — biggest
  knock-on win, native build link time drops because the
  `basis-universal-sys` C++ encoder no longer compiles.

**justfile** removals:

- The `bake-texarrays` recipe (lines 34-37) — 4 lines.

**lib.rs** removals:

- `pub mod baked_material;` (line 15).
- `pub mod texture_array;` (line 23).
- The two lines `baked_material::BakedMaterialPlugin,` (758) and
  `texture_array::TextureArrayPlugin,` (763) + their comment blocks
  (lines 753-757 and 759-762) — total ~14 lines of plugin-registration
  removal.

**Cargo workspace dependency drops (transitive)**:

- `basis-universal-sys` (a C++ encoder that links into native builds).
- The `bevy/asset_processor` + `bevy/basis-universal` Bevy features.
- All the `*.png.meta` `Load`-action sidecars become inert (they are
  irrelevant without the bake processor).

### Option B — keep the InstaMAT scaffold, drop the orphans

If the architect (or the user, on escalation) decides the InstaMAT
pre-bake pattern is load-bearing reference scaffolding the project wants
to keep on disk *as living code* rather than as a docs entry:

**Keep**:
- `src/bin/bake.rs` (96 LOC).
- `src/texture_array/{mod,def,loader,saver}.rs` (785 LOC).
- The `bake-texarrays` justfile recipe.
- The sample asset trio under `src/assets/textures/`.
- The native-only Cargo deps (`asset_processor`, `basis-universal`,
  `basis-universal = "0.3"`).

**Delete**:
- `src/baked_material.rs` (225 LOC) — its consumer (a `material.ron`
  load) never existed. The PBR design doc is the SSoT for how to rebuild it.
- `src/material_set/mod.rs` (60 LOC) — its consumer (any system reading
  `Res<MaterialSet>`) never existed. Same SSoT.
- `lib.rs:15` (mod decl), `lib.rs:753-758` (`BakedMaterialPlugin`
  registration).
- The `BakedMaterialPlugin`-only Cargo justifications above.

**Net Option B savings**: ~285 LOC + 14 LOC in `lib.rs` + the `MaterialRonLoader::extensions = ["ron"]` footgun.

### Recommendation

**Option A**. The user directive is unambiguous (*"everything else can go"*),
the PBR-raymarching design doc captures the InstaMAT pattern more
completely than the code ever did (a fully fleshed plan vs an empty
loader-shell), and the native-only `basis-universal` dependency adds real
native-build cost (~12 s clean compile per the user's typical iteration)
that nothing in the runtime currently pays back. If PBR ever resumes the
design doc is the canonical recipe.

The architect should still **escalate** Option A vs B to the user as
a single yes/no — the user directive's *"everything else"* might or
might not include the InstaMAT-pattern scaffolding the user explicitly
referenced in memory (`instamat-bake-to-disk.md`). Default-A if no
response.

---

## Tightening sketch (only if Option B chosen)

If the architect picks Option B (keep the bake binary + texture_array
loader), the surviving 881 LOC has minor idiom polish available:

- **`texture_array/loader.rs:120`** registers `&["texarray.ron"]` — fine
  on its own, but if `baked_material.rs` is also kept (Option B-extended
  / "keep everything"), fix `MaterialRonLoader::extensions` to
  `&["material.ron"]` (currently `&["ron"]`, which collides with every
  other `.ron` file in the tree as documented in *Bug spotted in the
  dead path*).
- **`bake.rs` `exit_when_finished` system** — uses a `Local<u32>` tick
  counter against a hardcoded `3_000` (lines 47-49). The 30 s cap is
  documented in prose but a named const (`MAX_BAKE_TICKS: u32 = 3_000;`)
  would self-document and make tuning a one-line edit. Severity: low.
- **`bake.rs` `ProcessorState`-not-`Debug` workaround** (lines 56-61) — a
  manual `match` formatting the `Some(state)` into a string. If Bevy
  ever lands `Debug` on `ProcessorState` upstream the helper goes away;
  for now it is the right amount of code. No action.
- **`saver.rs::compress_array_to_basis`** uses `Compressor::new(4)`
  (line 117) — magic number for "thread count". Named const + a
  doc-line ("UASTC4x4 has minimal threading benefit beyond 4 cores —
  see basis-universal README") would help future readers. Severity: low.
- **`loader.rs::bake_texture_array`** allocates `sources: HashMap<String, RgbaImage>`
  keyed by `String` (line 97). Every key is also a `ChannelSource::input` —
  borrowing `&str` would be the idiomatic move but the loader's
  `read_asset_bytes` consumes an owned `AssetPath<'static>` (per the
  comment at lines 104-105) so the ownership boundary is justified.
  No action.

None of these are architectural; they are at most ~10 LOC each. If
Option B is chosen the file group is already reasonably tight.

---

## Side notes / observations / complaints

1. **The `00-reuse-audit.md` D8 row is accurate but under-states the
   deadness.** It says *"This is essentially infrastructure-only code with
   no live consumer"* — correct — but stops there. The real situation is
   stronger: not only is the runtime-side `texture_array` loader dead,
   the `material_set` module is **completely unreachable** from `lib.rs`
   (its plugin is never added), and the files it tries to load **don't
   exist on disk** (`materials/{diffuse,normal,mrh,emissive}.texarray.ron`).
   The audit description ("plug-in registers loaders but ...") implies a
   wired-but-unused pipe; the reality is a literal orphan module that
   would `panic` on plugin-add because the asset paths it requests are
   absent. (It would not actually panic — Bevy's `AssetServer::load`
   returns a handle to a never-resolving asset — but functionally it is
   broken-as-shipped.)

2. **`material_set/mod.rs` is a paste of the PBR-raymarching design doc.**
   Compare `src/material_set/mod.rs:1-60` to
   `docs/orchestrate/pbr-raymarching/02-design.md:335-388` — they are
   character-for-character identical. The design doc proposed this module
   as the runtime shape; some past session pasted the proposal into `src/`
   without wiring it (the renderer hookup, the extraction system,
   `ExtractedMaterialSet`, the bind-group changes — none of those landed).
   The PBR effort either stalled or was deferred. This is the *exact*
   kind of "completed-port leaves scaffolding behind" rot the audit's
   side-note #4 calls out (Phase-A/B/C scaffolding), just in a different
   domain. **The architect should pick Option A unless the user
   explicitly says "preserve the PBR scaffold for future-Claude to find".**

3. **The `BakedMaterialPlugin` docstring at line 213 contains its own
   delete-me indicator**: *"infrastructure only … nothing in the scene
   is spawned or queried here. Wiring a baked material into a renderer
   is the consumer's job."* This is a code comment effectively saying
   "I have no consumer; the future consumer will wire me." The future
   consumer never showed up.

4. **The `MaterialRonLoader::extensions = &["ron"]` choice is wrong
   regardless** (footgun even if D8 stays). The fix is `&["material.ron"]`
   — Bevy's `AssetServer` does longest-extension matching so loaders
   that want `*.foo.bar` extensions register the full string. The
   loader's docstring even claims it (`baked_material.rs:96-99`:
   *"Registered for the `ron` extension — this repo has no other `.ron`
   assets, and a non-`MaterialRon` `.ron` would simply fail the RON
   parse here with a clear error"*) — that justification is brittle:
   it relies on no other developer ever adding a `.ron` asset. The
   audit's *"settings could grow `.ron` files later"* aside isn't
   speculative; this loader pre-emptively breaks that. Sub-point of
   findings #1/#4 — kept here because it is architecturally interesting
   even if the answer turns out to be Option A (deletion).

5. **InstaMAT-bake-to-disk user memory expects the bake binary to
   survive.** The `instamat-bake-to-disk.md` user memory notes
   *"justfile-driven, no AssetProcessor in production"* — and this is
   true and useful as a pattern reference. But the memory does not
   require the *code* to exist; it describes a pattern. The architect's
   escalation question to the user is therefore: *do you want the
   pattern reference to remain as living code, or is the docs entry
   (`docs/orchestrate/pbr-raymarching/00-reuse-audit.md:279-283`,
   `01-context.md:174,179,261,328` + the `02-design.md` `MaterialSet`
   sketch) enough?* If docs are enough → Option A. If you want the
   running template → Option B.

6. **Tests inside the modules cover ~100% of the D8 logic and would
   delete cleanly with their parents.** `loader.rs::tests` has 4 unit
   tests (lines 205-330), `saver.rs::tests` has 2 (lines 132-194). Both
   suites are well-scoped (pure-function tests of `bake_texture_array`
   and `compress_array_to_basis`) and would be removed together with
   the modules. No "orphaned test" risk.

7. **The image asset comments at `Cargo.toml:64-71` cite both
   `texture_array/loader.rs` (PNG/JPEG *decoding*) and `e2e/framebuffer.rs`
   (PNG *encoding*).** Deleting D8 still requires the `image` dep
   (e2e keeps PNG encoding); only the decoder side becomes unused, so
   `image = { ..., features = ["png"] }` (drop `"jpeg"`) is the
   tightened state — but the architect should verify with
   `grep -rn "image::jpeg\|JpegDecoder\|JpegEncoder" src/ e2e/` that no
   other site reads JPEG.

8. **D8 is the cleanest LOC-win in the entire codebase-tightening
   surface that has zero behavioural risk.** No production e2e gate
   exercises it. No CI step (only the optional `just bake-texarrays`
   developer command, never invoked unattended). Deleting the whole
   tree is a strict no-op for the runtime. Compare against
   D5's `validate_gpu_construction*` extraction (also a LOC win but
   *moves* code — moving 5 k lines is risky), or D7's `lib.rs` plugin
   pull-outs (risky because plugin ordering is load-bearing). **D8
   deletion is the safest LOC reduction in the entire orchestration**
   — but a small absolute win (1 199 LOC, ~2% of port).

9. **Subjective**: of the 8 domains, D8 is the only one where the
   "domain" almost doesn't deserve to exist as a refactor target —
   it's a deletion target. If the user signs off Option A, the
   implementor's brief reduces to a `git rm` list, 3-5 lines of
   `lib.rs` edits, and ~30 lines of `Cargo.toml` cleanup. The
   architect phase could plausibly skip a full `03-architecture.md`
   for D8 in favour of a 15-line `03-deletion-plan.md`. The
   orchestrator should consider whether D8 needs the full
   architect→implementor pipeline or whether a single "deletion
   implementor" dispatch off this exploration is enough.

10. **Out-of-scope rot worth flagging**: the `dist/src/assets/`
    artifact tree under `crates/bevy_naadf/dist/` mirrors the
    `src/assets/` tree as a Trunk build artifact. It is git-tracked
    (`ls -la` shows it), which is not ideal — `dist/` is generally
    a `.gitignore`-target. Not D8's job, but if D8 deletes the
    source `sample.texarray.ron` the mirrored copy in `dist/` will
    be a phantom file. Flag for D7 (app-and-camera) or whichever
    domain owns the build setup.
