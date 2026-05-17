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
- [ ] Checkpoint commit (Sonnet, commit-only)
- [ ] Consolidated dispatch (`delegate-consolidated`) — design + self-review + implement + log
- [ ] Hard-gate user review of `## Implementation log`
- [ ] (Conditional) Fresh-eyes `delegate-reviewer` if the self-review escalated any high-risk findings

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
