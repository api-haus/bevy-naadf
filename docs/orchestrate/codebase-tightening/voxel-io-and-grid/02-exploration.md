# D3 — voxel-io-and-grid — exploration

**Author**: refactor-explorer (codebase-tightening orchestration, D3 of 8).
**Date**: 2026-05-20.
**Scope**: `crates/bevy_naadf/src/voxel/{vox_import,cvox_import,web_vox,async_vox,voxel_dispatch,grid,mod}.rs` + `crates/voxel_noise/` (entire crate).

All file:line references verified with Read/Grep against the working tree at `main` (commit `e042b88`).

---

## Findings

Severity legend: **high** = architectural / IoC fault; **medium** = significant readability / coupling issue; **low** = worth flagging, low blast radius.

### F1 — `crates/voxel_noise/` is dead in this workspace and the user has explicitly green-lit deletion (severity: **high** — biggest LOC win in D3)

**Location**: `crates/voxel_noise/` (entire crate, 1 033 LOC across `src/{lib,native,presets,wasm_main}.rs` + `Cargo.toml` + `Makefile` + `js/voxel_noise_bridge.js` + `examples/{test_noise,test_ranges,test_simple_ranges}.rs`); workspace declaration at `Cargo.toml:16` (workspace root); docstring at `Cargo.toml:5-11`.

**Current state**: The workspace root `Cargo.toml:5-11` says verbatim *"Carried over from bevy_voxel_world; NOT yet wired into the renderer."* Grep across the entire `crates/bevy_naadf/src/` tree (excluding `target/`) for `voxel_noise` returns **zero hits in source**; the only references outside `crates/voxel_noise/` itself are documentation/orchestration docs (`docs/orchestrate/streaming-world/*`, `docs/orchestrate/codebase-tightening/00-reuse-audit.md`, etc.) and stale `target/` fingerprints. The crate is a sibling of `bevy_naadf`, depends on `fastnoise2` (a C++ FFI dep), and ships its own Emscripten build (`Makefile`, `js/voxel_noise_bridge.js`, `wasm_main.rs`). The brief states: *"USER DIRECTIVE: DELETE outright"* and the canonical `01-context.md` Q2 quotes the user: *"cpu oracle stays — without it we're blind when gpu yeets out, everything else can go"*.

**Why it's a problem**: 1 033 LOC of Rust + ~50 LOC of C-ABI/JS + a separate native-toolchain build path (FastNoise2 build-from-source pulls a C++ compiler dep) for code with zero callers. Every workspace `cargo build` rebuilds it. Every dependency audit and CI run includes the C++ toolchain because of it. Dead foreign-function-interface code is the worst kind of dead code: it bloats build matrices, gives reviewers a false sense of available capability, and forces every refactor that touches the workspace manifest to think about a crate nobody uses.

**Suggested direction (NOT a design)**: Delete the workspace member entry in `Cargo.toml:16` (`members = ["crates/bevy_naadf", "crates/voxel_noise"]` → `members = ["crates/bevy_naadf"]`), delete the docstring lines `Cargo.toml:5-11` that describe it, delete `crates/voxel_noise/` recursively, audit `justfile` / `Trunk.toml` / `scripts/` for any mention (grep already shows none in tracked source). The `streaming-world` orchestration that referenced this crate as a future dep stopped at the design phase (`docs/orchestrate/streaming-world/02-design.md`, last touched 2026-05-18); deleting now is fine — if streaming-world ever ships, it can re-add the dep from upstream or copy back the snapshot.

**Out-of-scope ripple**: `docs/orchestrate/streaming-world/{00-reuse-audit,01-context,02-design,README}.md` reference this crate by name as a future dependency. Architect should decide whether those docs get a "deleted — pull from upstream when needed" note or stay as-is (read-only history). `docs/orchestrate/pbr-raymarching/05-diagnostic.md:959` mentions a `voxel_noise` test pass-count in a verification log; pure history. The reuse-audit file itself (`docs/orchestrate/codebase-tightening/00-reuse-audit.md`) already lists this crate as deletable.

---

### F2 — `voxel/grid.rs::install_vox_sized_to_model` + entire `..._tiled` family in `vox_import.rs` only exist for one e2e gate and one unit test (severity: **high**)

**Location**:
- `voxel/grid.rs:365-403` — `install_vox_sized_to_model` (39 LOC).
- `voxel/vox_import.rs:161-164` — `parse_vox_bytes_tiled`.
- `voxel/vox_import.rs:184-187` — `load_vox_tiled`.
- `voxel/vox_import.rs:207-225` — `parse_dot_vox_data_tiled` (the actual body; `parse_dot_vox_data` at line 193 is now a one-line wrapper that always passes `tiles=1`).
- `voxel/vox_import.rs:235-276` — `replicate_buckets_xz` (~42 LOC).
- `lib.rs:403-409` + `lib.rs:456` — the `AppArgs.vox_gpu_oracle_cpu_phase` boolean flag.
- `e2e/vox_gpu_oracle.rs:283-289` — the SOLE place that sets the flag (`app_args.vox_gpu_oracle_cpu_phase = true;`).
- `voxel/grid.rs:127-138` — the dispatch branch in `setup_test_grid`.

**Current state**: The "tiled" feature lets a `.vox` parse replicate its model `tiles × tiles` times across XZ and ride the natural-bound CPU oracle install path. Confirmed call graph: production code always passes `tiles=1` (`parse_dot_vox_data` line 193: `parse_dot_vox_data_tiled(data, 1)`; `install_vox_sized_to_model` line 366: `load_vox_tiled(path, 1)`). The only `tiles>1` call site in the entire workspace is **one unit test** at `vox_import.rs:1685` (`parse_dot_vox_data_tiled(&data, 3)`) verifying that the dedup HashMap correctly collapses identical block content across tiles. `install_vox_sized_to_model` itself is reachable only via the `--vox-gpu-oracle` e2e gate's CPU-phase, gated through `args.vox_gpu_oracle_cpu_phase` which `e2e/vox_gpu_oracle.rs:289` sets to `true` and the production binary's CLI parser at `lib.rs:456` defaults to `false`. Stage 14 docs at `voxel/grid.rs:87-103` even acknowledge: *"Production callers never set `vox_gpu_oracle_cpu_phase`."* C# `MagicaVoxel.cs` has **no equivalent of any tiling code** (verified — grep for `replicate|tile|XZ|tiles|placement|MultiLoad|InstanceCount` in the C# source returns zero hits).

**Why it's a problem**: An entire structural seam (the "tiles" parameter threaded through 4 fns + the `replicate_buckets_xz` helper) exists to support one unit test. The `vox_gpu_oracle_cpu_phase` boolean further pollutes `AppArgs` (a production-facing struct) with a flag whose only setter lives in test code. This is the classic "test-induced API surface" anti-pattern: a test demands a tile knob → the knob propagates through 4 public functions → the entry-point fork becomes an `if args.test_flag { legacy_path } else { production_path }` in `setup_test_grid`. The faithful-port rule does not bind here — the tiling feature has no C# counterpart, so it's not faithful, it's a Rust-specific test-fixture branch.

**Suggested direction (NOT a design)**: The architect should consider one of three moves:
1. Inline `replicate_buckets_xz` into the test that needs it (the test can build a 3-tile `ChunkBuckets` by hand) and delete the tiles parameter from the entire `parse_*`/`load_*` family;
2. Keep `parse_dot_vox_data_tiled` as a `#[cfg(test)]` test-helper and collapse the production callers back to a single non-tiled entry point;
3. If the `--vox-gpu-oracle` CPU-phase is itself dead (architect: check `git log -- e2e/vox_gpu_oracle.rs` for last touch), the entire `install_vox_sized_to_model` + `vox_gpu_oracle_cpu_phase` flag can go with the same audit pass the orchestration brief allocates to D6.

**Out-of-scope ripple**: `e2e/vox_gpu_oracle.rs` (in D6), `bin/e2e_render.rs:343-347` (D6), `e2e/driver.rs:203,549,1525` (D6), `lib.rs:403-409,456` (D7). Architect must coordinate with D6 architect before proposing deletion of the gate itself; option 2 (cfg-test the helpers, keep the gate) is the move that doesn't cross domains.

---

### F3 — `voxel/grid.rs` install paths are 5 near-parallel monoliths, each rebuilding `WorldData`/`InitialCameraPose`/palette logging inline (severity: **medium**)

**Location**: `voxel/grid.rs:104-143` (`setup_test_grid` dispatch) + the five install fns:
- `install_empty_world` lines 164-228 (~64 LOC; only WebSkyboxOverride path)
- `install_default_embedded_in_fixed_world` lines 241-354 (~113 LOC)
- `install_vox_sized_to_model` lines 365-403 (~39 LOC; legacy CPU oracle, see F2)
- `install_vox_in_fixed_world` lines 422-436 + `install_vox_bytes_in_fixed_world` lines 463-478 (the bytes wrapper)
- `install_imported_vox` lines 529-663 (~134 LOC)

**Current state**: Each install fn redundantly constructs the same `WorldData { ..., bounding_box: full-fixed-world, pending_edits: Default::default(), dense_voxel_types: Vec::new(), block_hashing: BlockHashingHandler::new(), ... }` literal (lines 188-204, 304-330, 618-634 are 90% identical). Each also writes its own `[palette-install]` debug-log block (lines 214-226, 340-352, 647-661 — same `take(5).map(...).collect()` pattern repeated three times verbatim with only the `label` literal differing). Each computes and inserts its own `InitialCameraPose` differently (literal pose at lines 184-186, demo-relative pose at lines 293-302, horizon-pose at lines 570-574). The dispatch in `setup_test_grid` is a 5-arm `match` (lines 122-142) plus the `WebSkyboxOverride` short-circuit (lines 114-121).

**Why it's a problem**: Inverse-of-IoC: every new install scenario means a new fn that re-inlines all the boilerplate. The "DO NOT REMOVE — smoke detector for `web-vox-color-divergence`" comments (lines 213, 339, 646) are admissions that the three palette-install log blocks are deliberately kept in sync; they exist exactly to catch divergence between three things that shouldn't be three things. The C# pattern is one `WorldHandler.cs` that takes a `WorldGenerator` strategy; the Rust equivalent would be one builder fn that takes a `(ConstructedWorld, Vec<VoxelType>, InitialCameraPose, source_label)` and does the install once.

**Suggested direction (NOT a design)**: Extract a private `install_world(commands, constructed_world, palette, camera_pose, source_label)` helper that owns the `WorldData` construction + palette-install logging once. The five install fns then become small adapters that compute the (constructed_world, palette, camera_pose) triple and call the helper. The `WebSkyboxOverride` becomes either an `Empty` `GridPreset` arm or a regular `GridPreset::Empty` redirect. The end shape mirrors C#'s "one container, multiple generators" pattern.

**Out-of-scope ripple**: None inside D3 — the install fns are all `pub` only because the bytes-variant is reached from `web_vox.rs` (same domain) and `async_vox.rs` (same domain). External crates do not import them.

---

### F4 — `setup_test_grid` is doing scene-dispatch work that belongs in a Plugin / state-machine, not a `Startup` fn with an `Option<Res<>>` short-circuit (severity: **medium**)

**Location**: `voxel/grid.rs:104-143` (`setup_test_grid`).

**Current state**: The Startup system takes `Option<Res<WebSkyboxOverride>>` as a parameter and short-circuits to `install_empty_world` if present (lines 107, 114-121). Then it matches on `args.grid_preset` (line 122). Inside the `Vox` arm it again branches on `args.vox_gpu_oracle_cpu_phase` (line 127). Three independent decision axes (web-skybox-override, grid-preset, test-CPU-oracle-flag) compressed into one Startup body. The override resource is itself only inserted by `web_vox::startup_fetch_default_vox:404-411`, which has to be scheduled `.before(setup_test_grid)` (lib.rs:841) for the ordering to land.

**Why it's a problem**: The `.before(setup_test_grid)` Startup ordering is a brittle action-at-a-distance dependency. The `Option<Res<_>>` for "is this the skybox path?" leaks the *delivery mechanism* of the decision (resource-insert) into the decision-consumer's parameter list. Plus `AppArgs` carries `vox_gpu_oracle_cpu_phase` (a test-only flag, see F2). The whole thing is a state-machine-as-spaghetti: in C# this is one place that consults one `WorldGenerator` strategy field.

**Suggested direction (NOT a design)**: Architect should consider a `GridPreset` enum that already encodes all five outcomes (`Default`, `Vox{path}`, `Empty`, `WebSkybox`, `VoxSizedToModel{path}`) and a `Startup` system that just consults the enum. The wasm path then either:
1. Mutates `AppArgs.grid_preset` to `GridPreset::WebSkybox` from `startup_fetch_default_vox` (replacing the `WebSkyboxOverride` marker resource), or
2. Uses a `States` transition: `WorldLoadState::PendingScene → InstallingDefault → Ready` with the install firing on enter.

Either move removes the `Option<Res<_>>` parameter + the explicit `.before(...)` ordering dance.

**Out-of-scope ripple**: `lib.rs:840-841` (the `.before(setup_test_grid)` registration) and `lib.rs:824-825` (`PendingVoxParse` init). D7 owns `lib.rs`.

---

### F5 — `pin_web_horizon_camera` + `hide_ui` use `Option<Res<X>>` early-bail instead of `.run_if(resource_exists::<X>)` (severity: **medium**)

**Location**:
- `voxel/web_vox.rs:236-267` — `hide_ui` opens with `if override_resource.is_none() { return; }`.
- `voxel/web_vox.rs:273-296` — `pin_web_horizon_camera` opens with `if override_resource.is_none() { return; }`.
- `lib.rs:851-855` — `pin_web_horizon_camera` registered without a `run_if`.
- `lib.rs:970` — `hide_ui` registered without a `run_if`.

**Current state**: Both systems poll an `Option<Res<WebHorizonPoseOverride>>` / `Option<Res<UiHiddenOverride>>` parameter every `Update` and short-circuit when absent. Both markers are inserted exactly once at startup by `startup_fetch_default_vox:423,434` and never removed. So in production these systems run hot 60×/sec doing nothing in the common case (URL-param absent).

**Why it's a problem**: This is exactly the `BEV-6` pattern from `00-reuse-audit.md §3.3` (`Option<Res<X>>` ladder vs `.run_if(resource_exists::<X>)`). The Bevy idiom puts the existence check on the schedule, not in the system body — the scheduler skips the call entirely when the resource is missing. The current shape also makes the system harder to test (you can't drop the dependency type from a test harness without also changing the marker presence).

**Suggested direction (NOT a design)**: Register both systems with `.run_if(resource_exists::<WebHorizonPoseOverride>())` / `.run_if(resource_exists::<UiHiddenOverride>())` at `lib.rs:851-855,970`, and drop the `Option<Res<_>>` parameter — they become regular `Res<X>` consumers. This is mechanical, ~6 LOC saved, idiom-fit.

**Out-of-scope ripple**: `lib.rs:851-855,970` (D7 owns lib.rs). The system body refactor is in-scope D3.

---

### F6 — `web_vox.rs::apply_pending_vox` reaches across the cfg-gated module boundary to read `crate::e2e::vox_horizon_parity::{HORIZON_CAMERA_POS,HORIZON_CAMERA_ROT}` (severity: **medium**)

**Location**:
- `voxel/web_vox.rs:287-288` — direct import of `crate::e2e::vox_horizon_parity::HORIZON_CAMERA_POS` and `HORIZON_CAMERA_ROT` inside `pin_web_horizon_camera`.
- `voxel/grid.rs:571-572` — same import inside `install_imported_vox`, with a 12-line comment block (lines 556-568) explaining why this *production-binary* code path reaches into an `e2e` module for camera-pose constants.

**Current state**: The web path and the `.vox`-install path both hard-depend on constants defined in `e2e/vox_horizon_parity.rs`. The comments at grid.rs:556-568 acknowledge the awkwardness: *"the initial camera spawn pose now matches the cross-target SSIM gate's pose constants so a `just web-static` / `just web` / native release boot lands at the SAME camera the Playwright `vox-horizon-parity.spec.ts` screenshots."* So the production app is intentionally pinned to *e2e* gate poses, with the rationale being convenience (the user wants A/B comparison).

**Why it's a problem**: The e2e harness is supposed to consume production-side artifacts as a black box (per `01-context.md` rationale for D6 — e2e has *no C# counterpart by design*). When production imports *from* e2e, the dependency arrow inverts. Any refactor of `e2e/vox_horizon_parity.rs` (whose D6 audit suspicion list flags pin-camera duplication, DUP-6) now silently moves the production-binary camera. Worse: the `e2e/` module is conditionally compiled out under some test configs (verify this), which means production code may break under those.

**Suggested direction (NOT a design)**: Architect should move `HORIZON_CAMERA_POS` / `HORIZON_CAMERA_ROT` into a non-e2e module (`camera/poses.rs` or `voxel/grid.rs` next to `GRID_SIZE_IN_CHUNKS`) and have `e2e/vox_horizon_parity` import *from* there. Inverts the dependency arrow back. Tiny mechanical change once the destination module is chosen.

**Out-of-scope ripple**: `e2e/vox_horizon_parity.rs` (D6 owns it). D6 architect should coordinate — this is a "production wants to import a constant currently in e2e/" claim that D6 may already be planning to address via DUP-6's pin-camera consolidation.

---

### F7 — `parse_to_imported_vox`, `parse_voxel_bytes`, `parse_vox_bytes`, `parse_dot_vox_data` — a 4-level wrapper chain where each layer adds nothing but a docstring (severity: **low**)

**Location**:
- `voxel/grid.rs:502-513` — `parse_to_imported_vox(bytes: &[u8]) -> Result<ImportedVox, String>` — wraps voxel_dispatch + maps error to String.
- `voxel/voxel_dispatch.rs:80-91` — `parse_voxel_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxelParseError>` — magic-byte dispatch.
- `voxel/vox_import.rs:154-157` — `parse_vox_bytes(bytes: &[u8]) -> Result<ImportedVox, VoxImportError>` — `dot_vox::load_bytes` + `parse_dot_vox_data`.
- `voxel/vox_import.rs:193-195` — `parse_dot_vox_data(data: &dot_vox::DotVoxData) -> Result<ImportedVox, VoxImportError>` — wraps `parse_dot_vox_data_tiled(data, 1)`.
- `voxel/vox_import.rs:207-225` — `parse_dot_vox_data_tiled(...)` — the actual work.

**Current state**: A caller wanting "bytes → ImportedVox" walks `grid::parse_to_imported_vox → voxel_dispatch::parse_voxel_bytes → vox_import::parse_vox_bytes → vox_import::parse_dot_vox_data → vox_import::parse_dot_vox_data_tiled`. Five functions for one operation. Once F2's tile-deletion lands, that's six functions becoming five becoming three. Each layer has a substantial docstring explaining how it's "the magic-byte dispatch shim" / "the unit-testable entry point" / "the convenience over disk" / "pulled out so tests can drive it", but each is a one-liner.

**Why it's a problem**: Five-deep call stack to do `bytes → ImportedVox`. Each layer also adds error-mapping noise: the top layer maps to `String`, the dispatch layer unions two error types via `thiserror::#[from]`, the format-specific layer wraps `dot_vox::Error → &'static str → VoxImportError::Parse`. A new contributor reading this stack has to verify each shim is doing something non-trivial; they're not.

**Suggested direction (NOT a design)**: Once F2's tile deletion lands, `parse_dot_vox_data_tiled` can become `parse_dot_vox_data`. `parse_vox_bytes` collapses into `vox_import::parse_bytes` (just calls `dot_vox::load_bytes` + the parser). `parse_to_imported_vox` in `grid.rs` either inlines into its caller (`async_vox::spawn_native_vox_parse` etc.) or moves into `voxel_dispatch` and replaces the dispatch fn's `VoxelParseError` with a `String`-returning twin if string-return is genuinely a needed shape for callers.

**Out-of-scope ripple**: None — all wrappers are internal to D3.

---

### F8 — `voxel_dispatch::Cursor` (local name) collides with `std::io::Cursor` (cvox_import) — two parallel byte cursors, same name (severity: **low**)

**Location**:
- `voxel/cvox_import.rs:124` — `let mut cursor = Cursor::new(&payload);` resolves to the file-local `Cursor` struct (defined at lines 263-385).
- `voxel/cvox_import.rs:242` — `let reader = std::io::Cursor::new(bytes);` uses the std variant explicitly.
- `voxel/cvox_import.rs:263-385` — the hand-rolled `Cursor<'a>` byte cursor (with `read_i32`/`read_u32`/`read_f32`/`read_vec3`/`read_u32_array`/`read_null_terminated_string`).

**Current state**: Two cursors live in the same file under conflicting names; one is used by name (`Cursor::new`), the other by full path (`std::io::Cursor::new`). Reading `cvox_import.rs` linearly, the reader can't tell which is which without scanning the import list (`use std::io::Read;` at line 59).

**Why it's a problem**: Naming collision noise. The file's docstring even acknowledges the rationale (line 256: "We use a hand-rolled struct rather than `std::io::Cursor<&[u8]>` so the little-endian readers stay inline + non-allocating"). But the rename to `LeCursor` / `LeBytes` / `CvoxReader` would cost nothing and would eliminate confusion.

**Suggested direction (NOT a design)**: Rename the local struct to something descriptive (e.g. `LeReader<'a>` or `CvoxCursor<'a>`). Or — if the hand-rolled type's API matches what `byteorder::ReadBytesExt` / `binrw` provide for free — drop it for a third-party dep. The hand-rolled string reader (`read_null_terminated_string`, lines 372-384) is genuinely Latin-1-aware and is the only piece that doesn't have an off-the-shelf substitute.

**Out-of-scope ripple**: None — local to this file.

---

### F9 — `install_imported_vox` has a `let _ = WORLD_SIZE_IN_VOXELS;` no-op statement preserving a deleted code path's import (severity: **low**)

**Location**: `voxel/grid.rs:569`.

**Current state**: A bare `let _ = WORLD_SIZE_IN_VOXELS;` inside `install_imported_vox` (lines 556-568 explain the deleted code path it once belonged to: `InitialCameraPose::from_world_voxels`). The previous code used `WORLD_SIZE_IN_VOXELS` to compute a proportional pose; that path is now commented as "preserved on its function — only the call site here is overridden." The bare-binding statement keeps the import live for an explanatory comment.

**Why it's a problem**: A discarded local that exists solely to suppress an unused-import warning is a code smell — the reader has to figure out it's *deliberately* dead. The comment block at lines 556-568 is the load-bearing documentation; the `let _` is just clutter under it.

**Suggested direction (NOT a design)**: Drop the `let _` and let the unused import either compile-warn (then remove from the import list at line 34) or, if the comment block at 556-568 wants to gesture at the type, embed the name in the comment as text rather than as a live binding. Tiny mechanical clean.

**Out-of-scope ripple**: None.

---

## Confirmed / refuted audit suspicions

| brief suspicion | verdict | evidence |
|---|---|---|
| **(1) `vox_import.rs:1733 LOC` hand-rolls dot_vox scene graph collation — audit whether parts are our own additions or all faithful** | **partially refuted, partially confirmed.** The Rot3 / Xform / scene-graph walk pair (`accumulate_world_aabb` + `collate_voxels_sparse`) is a faithful port of C# `MagicaVoxel.GetWorldAABB` (`MagicaVoxel.cs:651-716`) + `MagicaVoxel.CollateVoxelData` (`MagicaVoxel.cs:718-755`). The size delta vs C# `MagicaVoxel.cs:757 LOC` is real but explained by: (a) C# delegates the RIFF chunk parser to its own embedded reader, ours delegates to `dot_vox` (RIFF parsing not in the line-count), (b) we have a 700-LOC `mod tests` block (lines 1010-1733), (c) the sparse `ChunkBuckets` + `build_constructed_world_sparse` pipeline (lines 524-953) is genuinely ours — C# emits via `DenseVolume`-equivalent. **NEW finding from this audit**: the entire `tiled` family (F2) is a Rust-specific addition for one test. Otherwise the audit-suspicion's worry ("we have 2× the C# LOC") is mostly justified by genuine work (sparse path + tests). | Read of vox_import.rs lines 1-953; grep of MagicaVoxel.cs for `tile|replicate|XZ` returned zero hits. |
| **(2) `grid.rs:1354 LOC` install paths can collapse now that vox-gpu-rewrite Stage 2 consolidated** | **confirmed in part — see F3.** The dispatch (`setup_test_grid` lines 104-143) is genuinely small now; what's bloated is the per-install-fn boilerplate (lines 188-204 ≈ lines 304-330 ≈ lines 618-634 are three near-identical `WorldData { ... }` literals). Plus the three palette-install debug-log blocks (lines 214-226, 340-352, 647-661) are deliberately-synced duplicates. F3 is the consolidation move. **Refuted**: the install branches themselves aren't the rot — they each genuinely do different things (`install_default` builds a synthesised dense scene, `install_imported_vox` consumes a parsed `ImportedVox`, `install_empty_world` skips both); collapsing them would lose intent. The shared `install_world` helper recovers the LOC without losing the intent. | Read of grid.rs lines 104-663. |
| **(3) `crates/voxel_noise/` is dead — verify zero callers from `bevy_naadf`** | **confirmed — F1.** Grep across `crates/bevy_naadf/src/` (excluding `target/`) for `voxel_noise` returns zero source hits. The `streaming-world` orchestration that plans to use this crate stopped at the design phase 2026-05-18 and has no impl. User directive is explicit: delete. | Grep + `git log` for `crates/voxel_noise/` and `docs/orchestrate/streaming-world/`. |
| **BEV-5 partial — `web_vox.rs::apply_pending_vox` polls `PendingVoxParse` every `Update`, could be `Added<>` / `Changed<>`** | **refuted as stated; reframed.** `poll_pending_vox_parse` and `apply_pending_vox` genuinely *must* run every frame: the async future may complete on any tick after dispatch, and `Added<T>`/`Changed<T>` fire only on insert/mutation, not on async-completion. So those two systems are correctly polling. **However** the related markers `WebHorizonPoseOverride` / `UiHiddenOverride` and the systems consuming them (`pin_web_horizon_camera`, `hide_ui`) DO match the BEV-6 (run_if) idiom and are the right targets — see F5. | Read of `apply_pending_vox` lines 498-549 + `poll_pending_vox_parse` lines 81-153 vs the marker-driven systems lines 236-296. |

---

## Side notes / observations / complaints

1. **`compose_default_scene_into_fixed_world` (grid.rs:815-911) is a 96-line "pointer-shift" composer with substantial domain logic and a 16-line docstring explaining the BlockPtr-sharing invariant.** Not in any audit suspicion but worth flagging as a side note: it's load-bearing (every default-scene boot runs through it) and the docstring lines 793-814 + 829-830 are the only test of the invariant outside the unit-tests at lines 1230-1312. Architect may consider whether the "every ground chunk owns a unique BlockPtr" invariant deserves a `debug_assert!` at line 893 rather than only living in the doc + a tail-end test. Out of D3's primary scope but the next refactor session that touches `aadf/cell::BlockPtr` semantics could break this silently.

2. **`voxel_dispatch.rs` is a clean, small, well-tested module (168 LOC).** This is what good D3 code looks like. It deserves to be the template for what F3's `install_world` helper extraction should look like. Worth flagging because the rest of the D3 surface could plausibly compress toward this density.

3. **The PR-orchestration leaves an audit trail of "DO NOT REMOVE" markers across `grid.rs` (lines 213, 339, 646) for the `[palette-install]` log blocks.** They're keeping three near-identical debug-log statements in sync to detect the `web-vox-color-divergence` regression. The right move (architect's call) is to extract the palette-install log into the helper that F3 proposes — then there's *one* statement and *one* place that can ever drift. The "DO NOT REMOVE" markers exist precisely because the code shape currently demands hand-synced duplicates.

4. **The brief flagged "wasm async pump is a deliberate divergence from C#" — confirmed, and the divergence is well-shaped.** `async_vox.rs` is cfg-gated cleanly (lines 43-63 split native `Task<>` from web `crossbeam_channel::Receiver<>`); the `PendingVoxParse` resource has a uniform surface to consumers. The `web_vox.rs::apply_pending_vox`'s two-stage deferred parse dance (lines 498-549) exists for the legitimate reason that the browser needs a paint between "bytes landed" and "rayon dispatch starts" so the "Parsing..." overlay shows. That's not a Rust idiom miss; that's a wasm-platform necessity. Architect should keep it.

5. **The `voxel/mod.rs` (144 LOC) is the **bit-layout constants** + the `VoxelType`/`MaterialBase`/`MaterialLayer` definitions, NOT a voxel-IO orchestrator.** D1's brief in `00-reuse-audit.md §2 D1 row` claims `voxel/mod.rs:145` is dependency-root for `aadf/` and lives in D1, NOT D3 — this is a domain-boundary clarification: any D3 refactor touching the bit-layout constants would actually touch D1's surface. Verified my refactor candidates do not touch `voxel/mod.rs` constants.

6. **The `streaming-world` orchestration in `docs/orchestrate/streaming-world/` (5 docs, stopped at design phase 2026-05-18) explicitly designs around using `voxel_noise` as a future dep.** If the user revives that orchestration after this refactor lands `voxel_noise` deletion, the streaming-world design doc will need a "pull from upstream" note added. Not blocking; just a paper trail to keep clean. Worth telling the architect so they know what F1's deletion cascades into.

7. **One subjective reaction**: `voxel/grid.rs` reads like a museum of historical orchestration phases — vox-gpu-rewrite Stage 2 docs at lines 73-103, web-vox-async-loading Step 9 docs at lines 109-121, Stage 14 at lines 86-100. The docstrings are excellent for archeology but they're stacked, not synthesised. The architect could consider whether the next refactor pass also gets to *remove* the Stage-N narrative once the corresponding code stabilises — the in-tree archeology can move to `docs/architecture.md` and the code can become legible without 50 lines of prelude. Out of D3's narrow IoC/refactor scope, but it's the kind of thing that makes the file feel bigger than it is.

8. **`install_imported_vox`'s 134-LOC body (grid.rs:529-663) is mostly logging.** Strip the four `info!/debug!` blocks (lines 535-554, 638-661) and the AADF-strip comment block (lines 576-589) and the function is ~30 LOC of actual work. The logging is load-bearing for the `web-vox-color-divergence` regression (per the "DO NOT REMOVE" markers above), so it stays — but architect should recognise that this fn's *visible size* is dominated by debug instrumentation, not by complexity. Comparison helps frame which install fns genuinely need decomposition vs which just need the logging extracted.

9. **Equal-footing observation**: the brief lists the user directive on `voxel_noise` as binary ("DELETE"). My audit confirms that — but I want to flag that the deletion irreversibly takes us off the path the `streaming-world` orchestration was designing. The user's quote ("everything else can go") is unambiguous; this side note is just so the next architect/implementor knows what's being foreclosed, not asking for a re-decision.

---

## Open questions for the architect

- **F2's blast radius into D6.** Architect should coordinate with D6 architect: if D6 plans to delete the `--vox-gpu-oracle` CPU phase entirely (along with `pbr_*` and other investigation-residual gates), then F2's `install_vox_sized_to_model` + `vox_gpu_oracle_cpu_phase` flag + the entire `tiled` family go with it. If D6 keeps the gate, F2's option 2 ("`#[cfg(test)]`-fence the helpers") is the in-domain move.
- **F6's destination module for the horizon camera constants.** Architect must pick a non-e2e home (`camera/poses.rs` is a candidate, `voxel/grid.rs` next to `GRID_SIZE_IN_CHUNKS` is another). The decision is small but should be confirmed before D6's architect touches `e2e/vox_horizon_parity.rs`.
- **F1's `streaming-world/*` docs.** Architect should decide whether to leave them as-is (read-only history, deletion is mentioned in their `voxel_noise` references), or add a "this dep was deleted on 2026-05-20 — see `00-reuse-audit.md`" stub. Either is defensible; just needs a call.
- **F4's preferred shape: enum-extension vs Bevy `States`.** Architect should pick between (a) extending `GridPreset` to `{Default, Vox{path}, Empty, WebSkybox, VoxSizedToModel{path}}` and removing `WebSkyboxOverride` + `vox_gpu_oracle_cpu_phase`, vs (b) modelling install progression as a `States` transition. Option (a) is smaller (just enum work + Startup-system simplification); option (b) is more idiomatic Bevy but introduces a new mechanism. User Q1 says "idiom-fit first, LOC reduction is consequence" — that mildly favours (b), but (a) is fine.
