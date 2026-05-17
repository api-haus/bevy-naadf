# `web-chunks-storage-buffer` orchestration index

Migrate the `chunks` 3D `Rg32Uint` storage texture to a flat
WebGPU-compliant storage buffer (`array<vec2<u32>>`) so the web build's
construction bind-group layouts stop tripping WebGPU validation and the e2e
smoke test passes.

**Execution mode:** consolidated — one `delegate-consolidated` agent in a 1M
Opus window, design → self-review → implement → log in one uninterrupted
run. Eligible per all four Step 2.5 criteria: bounded context (15 files
named by the audit), single cohesive scope, low blast radius / reversible
(committed checkpoint + 184 lib tests + 9 e2e modes as safety net), tight
design↔impl coupling.

## Files

| file | purpose | status |
|---|---|---|
| `00-reuse-audit.md` | Existing storage-buffer patterns, `flatten_index` helper, W4 design-doc trace, fixture-site inventory. Written by `delegate-auditor`. | `[x]` complete |
| `01-context.md` | Canonical context bundle the consolidated agent reads on entry: goal, Q&A decisions, audit summary, required reading with line ranges, forbidden moves. | `[x]` complete |
| `02-design-impl.md` | Consolidated agent's output: `## Design`, `## Decisions & rejected alternatives`, `## Assumptions made`, `## Independent review`, `## Implementation log`. Flushed in stages during the dispatch. | `[ ]` pending |

## Phase checklist

- [x] Re-implementation audit (`00-reuse-audit.md`)
- [x] Mode selection — consolidated (Step 2.5, user-confirmed at Step 4)
- [x] Architectural Q&A (Step 4) — 4 decisions captured in `01-context.md`
- [x] Context bundle (`01-context.md`)
- [x] Checkpoint commit (`8c2fd63`)
- [x] Consolidated dispatch — design + self-review + implement + log written to `02-design-impl.md`; 9 of 10 verification gates PASS, 1 FAIL (`just test-wasm-full`, residual `DeviceLost` with no surfaced WebGPU validation error). **Chunks migration committed at `b1de4ef`.**
- [x] e2e smoke-test fidelity bump — 5 s → 10 s wait + canvas screenshot capture (`test-results/.../canvas-after-10s.png`). Result: 10 s wait alone surfaced no new errors (DeviceLost terminates first), but the screenshot mechanism + the headed-mode pivot below paid off.
- [x] **Headed-mode re-run** — `just test-wasm-headed`. **Three new actionable validation errors surfaced** (masked by the headless `DeviceLost`): `naadf_map_copy_pipeline::copy_map` Invalid ShaderModule, `naadf_generator_model_pipeline::fill_chunk_data_with_model_data_16` **missing entry-point**, `naadf_map_copy_test_hash_pipeline::test_hash` Invalid ShaderModule. Screenshot 1.85 MB — live framebuffer.
- [x] **Test improvements committed** at `d8b7d84`.
- [x] **Validation-error fix dispatch** — 4 mechanical edits, all 3 named errors resolved. (1) `map_copy.wgsl` `old_map` storage promoted `read` → `read_write` because WGSL spec forbids `read`-mode storage containing `atomic<u32>` (Dawn strict, naga lenient — the kernel never writes to `old_map`, change is layout-only). (2) Matching layout entry in `map_copy.rs` flipped to `storage_buffer_sized`. (3) `generator_model.wgsl` entry-point renamed `fill_chunk_data_with_model_data_16` → `fill_chunk_data_with_model_data` (trailing `_16` digit-suffix triggered a naga WGSL-writeback rule producing an empty shader module). (4) Matching Rust caller in `generator_model.rs` updated. All 5 Phase-3 gates green except `just test-wasm-full` (residual pre-existing headless-only `DeviceLost`, unrelated to the named errors). **Fix is uncommitted in the working tree.**
- [ ] **Hard-gate user review** ← pending; decide between (a) commit the fix and stop (named goal + named follow-ups all resolved), (b) commit the fix and continue into the headless `DeviceLost` (separate orchestration territory), (c) commit the fix + the trivial 404-favicon cleanup, then stop.

## Decisions captured

1. **Execution mode: consolidated.** One continuous trace; design space is
   tightly constrained by the audit, so a distributed design phase would
   mostly transcribe the audit.
2. **Fixture scope: all 5 sites lockstep.** Production + 4 test fixtures
   flip in one dispatch; `cargo test` stays green at the end.
3. **Verification: full gate quartet.** `cargo test --workspace --lib`,
   `just web-build`, `just test-wasm-full` (the load-bearing WebGPU gate),
   `cargo run --bin e2e_render -- <mode>` (at minimum `baseline`,
   `--validate-gpu-construction`, `--edit-mode`, `--entities`,
   `--oasis-edit-visual` to cover the chunks-touching dispatch paths).
4. **Stride source: `world_meta.size_in_chunks` (read inline at each call
   site).** No new uniform fields; matches how `chunk_calc.wgsl:347`
   already reads `params.segment_size_in_chunks`.
