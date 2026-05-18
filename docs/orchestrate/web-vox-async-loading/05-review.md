# 05-review — fresh-eyes review brief

**You (the reviewer) are deliberately working without the design rationale or the orchestration context.** Do **not** read `01-context.md` or `03-architecture.md`. Do **not** ask why a decision was made — only whether the success criteria are met. The orchestrator reconciles your flags against the design rationale; that reconciliation is its job, not yours.

You read:
1. This file (`05-review.md`) — success criteria + artifact pointer.
2. The implementer's verification log `docs/orchestrate/web-vox-async-loading/04-refactoring.md` — what changed, what was verified, what gates ran.
3. The actual code diff vs `origin/main` for branch `feat/web-vox-streaming` (use `git diff origin/main...HEAD` in `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`).
4. The new / modified source files cited by the implementer's log.

You do **not** read `00-reuse-audit.md`, `01-context.md`, or `03-architecture.md`.

## Success criteria — verify each independently

### S1 — async parse, web

The web build's `.vox` install does **not** block the wasm main thread for the duration of `dot_vox` parsing. Evidence accepted:

- The actual parse call (`parse_vox_bytes` or equivalent — the call that produces `ImportedVox` from the fetched bytes) executes off the main thread (rayon thread pool, dedicated worker, or task pool with a real worker backend on wasm32).
- The `#loading` overlay (with `#progress-fill.indeterminate` + `#progress-text` "Parsing model…") remains responsive during parse — confirm by reading the Playwright spec's wait assertions and the Rust code that drives the overlay.
- The browser UI does **not** freeze during the parse. The implementer's verification log must include evidence the Playwright spec passed headed.

Reject if: parse runs synchronously on the wasm main thread; if `#loading.hidden` is not toggled correctly across parse start/finish; if any "skip on web" path exists.

### S2 — async parse, native

Native `.vox` load (both `Startup` boot-time load and `native_vox_drop_listener` drag-drop) does **not** block the main thread for the duration of `dot_vox` parsing. Same shape of evidence as S1 but for native.

Reject if: native `Startup` system synchronously calls `parse_vox_bytes` inline; if `native_vox_drop_listener` still does inline `std::fs::read` + parse synchronously inside the event handler.

### S3 — async GPU readback works on BOTH web AND native

`populate_cpu_mirror_from_gpu_producer` reads back via a real async path that works on both WebGPU and native wgpu. Evidence accepted:

- The interim wasm32 short-circuit at `crates/bevy_naadf/src/render/construction/mod.rs:944-957` is **deleted** (the entire `#[cfg(target_arch = "wasm32")]` block in `populate_cpu_mirror_from_gpu_producer`).
- The replacement readback path does NOT contain `Device::poll(PollType::wait_indefinitely())` on the WebGPU path. Cross-frame state machine, oneshot-channel-await, or AsyncComputeTaskPool-driven `.await` are all acceptable; sync `poll(wait_indefinitely)` followed by `get_mapped_range` is **not**.
- After the readback completes, the CPU mirror is populated. The editor's hash-keyed `set_voxel*` operations (which require the CPU mirror) function correctly on web — this is asserted by the new e2e gate.

Reject if: any "skip readback on web" code path remains; if the readback path has a wasm32-only branch that diverges from native semantics; if any production-path source buffer that the readback reads from is missing `COPY_SRC` (verify via grep on `crates/bevy_naadf/src/render/construction/mod.rs` and `crates/bevy_naadf/src/world/buffer.rs`).

### S4 — new native e2e gate exists and passes

A new gate (working name `--vox-web-parity`, but accept any name implied by the implementer's log) exists in `crates/bevy_naadf/src/bin/e2e_render.rs`. Verify:

- The gate boots in `AppConfig::e2e` mode.
- Loads `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` via the async native path (not synchronously).
- Captures a skybox-only baseline framebuffer PNG AND a post-`.vox`-install framebuffer PNG to `target/e2e-screenshots/`.
- SSIM-compares the two PNGs via `image-compare`'s `Algorithm::MSSIMSimple` (per the existing `vox_gpu_oracle.rs` template).
- **Asserts SSIM < threshold** (dissimilarity — opposite of the existing `vox_gpu_oracle.rs` which asserts SSIM ≥ threshold for similarity).
- Asserts zero pipeline errors AND (per the handoff) zero tracing-level errors during the run.
- The implementer's `04-refactoring.md` shows the gate passing.

Reject if: the gate name is missing; if the skybox baseline phase doesn't actually render an empty/sky-only scene; if the SSIM threshold is hardcoded without explanation; if the gate has any `while !condition { yield }` loop without a wall-clock budget AND diagnostic-bail on exhaustion (per `feedback-e2e-gates-must-fail-fast.md`).

### S5 — Playwright spec extended with SSIM gate

`e2e/tests/vox-loading.spec.ts` is extended to:

- Capture a baseline skybox-only screenshot (boot with a flag forcing empty scene — query string or equivalent).
- Capture a post-`.vox`-install screenshot.
- SSIM-compare by shelling out to a Rust binary wrapping `image-compare` (NOT by adding a Node SSIM lib).
- Asserts SSIM < threshold.
- Asserts zero `console.error` / Bevy ERROR / page-error events and zero wasm panics.
- Always headed (`channel: "chrome"`, system Chrome; no headless "fix").
- The implementer's log shows `just test-wasm` passing.

Reject if: any Node SSIM library (`ssim.js`, `image-ssim`, `pixelmatch`) was added instead of the shell-out; if the spec runs headless by default; if the spec asserts only `ok` events without the SSIM dissimilarity check.

### S6 — verification log completeness

`docs/orchestrate/web-vox-async-loading/04-refactoring.md` shows ALL of the following gates passing in the dispatched-implementer run:

- `cargo build --workspace` ✓
- `cargo build --target wasm32-unknown-unknown --bin bevy-naadf --no-default-features --features webgpu` ✓ (or whichever wasm flags the trunk build uses)
- `cargo test --workspace --lib` ✓
- `cargo run --bin e2e_render -- --vox-web-parity` (or the new gate's name) ✓
- `cargo run --bin e2e_render -- --vox-e2e` ✓ (regression check)
- `cargo run --bin e2e_render -- --oasis-edit-visual` ✓ (regression check — canonical visual gate)
- `just test-wasm` ✓ (headed Playwright)
- Attached: the captured baseline-skybox.png and vox-loaded.png as proof of real SSIM dissimilarity (not zeros, not identical).

Reject if any gate is missing, or is shown as "skipped" / "not run" / "not applicable" without justification, or if `cargo run --bin bevy-naadf` was used as a verification step (project rule forbids that).

### S7 — no scope widening

The change set is bounded:

- New + modified files only in the areas listed by the implementer.
- No rewrite of existing e2e gates other than additive registration in `add_e2e_systems` / driver phase enum.
- No removal of non-Q7 code.
- No new dependencies beyond what S1's async-on-web route requires (e.g. `wasm-bindgen-rayon`) and S5's Rust SSIM binary requires.

Reject if: the diff touches unrelated subsystems (rendering, world systems, naadf params) without justification; if entire existing gates were rewritten.

## Deliverable shape (your output)

Append to `docs/orchestrate/web-vox-async-loading/05-review.md` under section heading `## delegate-reviewer findings (<ISO date>)` with the structure:

```markdown
## delegate-reviewer findings (YYYY-MM-DD)

### Summary
<one-line verdict per S1–S7; e.g. "S1 ✓ / S2 ✓ / S3 ✗ — see flag #2 / S4 ✓ / S5 ✓ / S6 ✓ / S7 ✓">

### Flags (numbered)
1. <criterion>: <what is wrong, citing file:line>
2. ...

### Out-of-scope observations
<things you noticed that aren't review failures but the orchestrator may want to know about>

### Verification commands you ran
- `git diff origin/main...HEAD -- <path>` — ...
- `grep -n "<pattern>" <file>` — ...
- ...
```

## Hard rules for the reviewer

- Do NOT read `01-context.md`, `03-architecture.md`, or `00-reuse-audit.md`. The orchestrator's job is reconciling your fresh-eyes flags against the rationale you don't see.
- Cite file paths with line numbers, not paraphrases.
- Reject criteria you cannot verify — do not assume the implementer's log is accurate. Spot-check at least the new gate's source code and the readback's wasm path.
- Do not propose fixes — your output is "what is wrong + where", not "what to do about it".
- Do not run builds or tests yourself; the implementer's log is the verification artifact you're reviewing.
