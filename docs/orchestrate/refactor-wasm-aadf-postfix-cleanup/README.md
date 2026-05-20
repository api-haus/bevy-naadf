# refactor-wasm-aadf-postfix-cleanup

## Goal (one-line)

Structural cleanup of the wasm-chunk-aadf-nondeterminism fix's artifacts —
incoherent narrative in `naadf_bounds_compute_node`, the 18% parity gap on
the chunks-RMW pattern, and the drifted test/production probe-buffer
hardcode.

## Worktree

- Path: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-aadf-postfix-cleanup`
- Branch: `refactor/wasm-aadf-postfix-cleanup`
- Branched from: main `1fdd256` (post the four commits that landed the
  wasm bug fix + cleanup: `a426441 + 960eeb2 + c6b0deb + 1fdd256`).

## Files

| File | Owner | Status |
|------|-------|--------|
| `README.md` | orchestrator | [x] |
| `01-context.md` | orchestrator | [x] |
| `02-exploration.md` | `refactor-explorer` | [ ] |
| `03-architecture.md` | `refactor-architect` | [ ] |
| `04-refactoring.md` | `refactor-implementer` | [ ] |

## Phase checklist

- [x] Exploration — 11 findings written (7 Item 1, 3 Item 2, 1 Item 3 + cross-cutting + side-notes)
- [x] User confirmed dispatching architect on all 11 findings
- [x] Architecture — 5 migration steps; Item 1 = 7 findings via steps 2-5; Item 2A → ESCAPE; Item 2B/2C → EXPLORE-ONLY; Item 3 = mechanical (step 1)
- [ ] User confirms the design
- [ ] Refactoring — apply edits + run verification gates

## Scope items (three)

1. **`naadf_bounds_compute_node` cleanup** in `crates/bevy_naadf/src/render/construction/bounds_calc.rs`. The function works correctly but has accumulated layered iter-N intervention comments from the brute-force fix-finding session. **Restructure latitude: comments + control-flow tightening** (user-confirmed Q&A option B). Docblock rewrite + minor control-flow tightening (collapse dead `let _ = ...` patterns, consolidate cfg branches, extract natural-boundary sub-blocks into helpers). All behavior preserved; verified via 3-run e2e gate.

2. **chunks-RMW pattern + 18% parity gap.** Multiple agents flagged the cross-workgroup RMW on `chunks[]` as fundamentally GPU-cache-unfriendly. Web parity to CPU oracle is only ~18% even with the full fix. User has empirically validated no visual disparity in fly-through across previously-broken angles. **Commit policy: small+obvious+low-risk fixes are allowed** if the architect surfaces them (user-confirmed Q&A option B). Larger restructures escape to a separate session via architect's recommendation.

3. **`crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:529` probe-buffer hardcode.** Currently `2048 * 16` bytes; production const `PREPARE_PROBE_HISTORY_ENTRIES` is now 256 (downsized in commit `c6b0deb`). Align the test with the production const to prevent future divergence. **Restructure latitude: full** — this is a pure mechanical alignment.

## Non-negotiable constraints

- `HORIZON_SSIM_SIMILARITY_MIN = 0.91` stays at 0.91.
- `MAX_RAY_STEPS_PRIMARY` stays at its current value.
- `WASM_MAX_GROUP_BOUND_DISPATCH = 4096` stays at 4096.
- `n_bounds_rounds = 1` wasm clamp in `From<&AppArgs>` stays (it is THE load-bearing fix per docs 13/14).
- `chunks_mirror` infrastructure stays in place as a mechanism — but item 1 may RESTRUCTURE it (rename, extract into helper, etc.) provided semantics are preserved.

## Verification gates

For item 1 (control-flow changes) and item 3 (test alignment):
- `cargo check --workspace` — clean.
- `cargo test -p bevy-naadf --lib` — passes.
- `cargo run --release --bin e2e_render -- --vox-horizon-native` — passes (≥2 runs, deterministic native baseline).
- `cd e2e && timeout 240s npx playwright test vox-horizon-parity.spec.ts --headed` — passes SSIM ≥ 0.91 on **all 3 of 3 runs** (the wasm multi-run discipline).

For item 2 (explore-only OR small low-risk fix):
- Explore-only deliverable lands in `03-architecture.md` as a "Item 2: parity-gap analysis" section.
- If the architect's analysis surfaces a small+obvious+low-risk fix qualifying for the commit-policy, the implementer applies it and runs the same gates above.

## Orchestration lineage

This refactor session is a follow-up to the wasm-chunk-aadf-nondeterminism
`/delegate` orchestration which landed the bug fix across four commits
(`a426441 + 960eeb2 + c6b0deb + 1fdd256`). The accumulated context lives
at `docs/orchestrate/wasm-chunk-aadf-nondeterminism/` (15 numbered
artifacts). The refactor docs in `docs/orchestrate/refactor-wasm-aadf-postfix-cleanup/`
are a separate folder; they reference but do not duplicate the prior
orchestration's findings.
