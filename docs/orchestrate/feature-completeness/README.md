# feature-completeness — orchestrate group

Distributed-mode `/delegate` orchestration covering two feature-completeness
tracks (the **GPU algorithmics** track was dropped — already landed in Phase C
per `12-alignment-gap.md` rows 17–21).

## Files

| File | Purpose | Status |
|---|---|---|
| `README.md` | This index. Phase checklist. | live |
| `00-reuse-audit.md` | Re-implementation audit, both tracks. | **complete** (delegate-auditor, 2026-05-15) |
| `01-context.md` | Canonical context bundle — every non-review agent reads this first. | **complete** |
| `02a-design-vox-loading.md` | Track A design — `.vox` import path. | **complete** (delegate-architect, 2026-05-15) |
| `02b-design-editor.md` | Track B design — paint/cube/sphere + Bevy-UI. | **complete** (delegate-architect, 2026-05-15) |
| `03a-impl-vox-loading.md` | Track A implementation log. | **complete** (general-purpose Opus, 2026-05-15) — uncommitted on disk |
| `03a-followup-empty-scene-diagnosis.md` | Track A follow-up — diagnose + fix empty-scene + camera-dark on real `.vox` file; lifted Decision-6 identity-only walk. | **complete** (general-purpose Opus, 2026-05-15) — uncommitted on disk |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | Track A E2E gate addendum — synthesised-fixture `--vox-e2e` mode + `assert_vox_geometry_visible` non-skybox gate. Logged in `03a-impl-vox-loading.md` `## E2E gate addendum`. | **complete** (general-purpose Opus, 2026-05-15) — uncommitted on disk |
| `03b-impl-editor.md` | Track B implementation log. | pending |
| `04a-review-vox-loading.md` | Track A fresh-eyes review brief + verdict. | pending |
| `04b-review-editor.md` | Track B fresh-eyes review brief + verdict. | pending |

## Agent groups

- **audit** (one-shot, complete) — `delegate-auditor`. Output: `00-reuse-audit.md`.
- **design-vox** — `delegate-architect`. Reads `01-context.md` + `00-reuse-audit.md`. Writes `02a-design-vox-loading.md`.
- **design-editor** — `delegate-architect`. Reads `01-context.md` + `00-reuse-audit.md`. Writes `02b-design-editor.md`.
- **impl-vox** — `general-purpose` (Opus). Reads `01-context.md` + `02a-design-vox-loading.md` (incl. `## Decisions & rejected alternatives` + `## Assumptions made`). Writes `03a-impl-vox-loading.md`.
- **impl-editor** — `general-purpose` (Opus). Reads `01-context.md` + `02b-design-editor.md` (incl. decisions + assumptions). Writes `03b-impl-editor.md`.
- **review-vox** — `delegate-reviewer`. Reads **only** `04a-review-vox-loading.md`. Writes verdict to same file.
- **review-editor** — `delegate-reviewer`. Reads **only** `04b-review-editor.md`. Writes verdict to same file.

## Phase checklist

- [x] Step 1 — scope + topic-slug pick
- [x] Step 2 — re-implementation audit (`00-reuse-audit.md`)
- [x] Step 2.5 — mode selection (distributed, parallel fan-out at design)
- [x] Step 3 — present method to user
- [x] Step 4 — architectural Q&A (K-means impl, obj2voxel posture, set_voxel batching)
- [x] Step 5 — shared-context files (`README.md`, `01-context.md`)
- [x] **Step 6a** — checkpoint + dispatch `design-vox` (parallel with `design-editor`)
- [x] **Step 6b** — checkpoint + dispatch `design-editor` (parallel with `design-vox`)
- [x] **Step 7a** — synthesis after design phase, hard gate
- [x] **Step 8a** — checkpoint + dispatch `impl-vox`
- [x] **Step 7b** — synthesis after `impl-vox`, hard gate
- [x] **Step 8b-followup** — checkpoint + dispatch `diagnose-empty-scene` (user-directed; scene-graph composition fix landed)
- [x] **Step 8b-e2e-test** — checkpoint + dispatch `impl-vox-e2e-test` (user-directed; automated .vox-render gate). `--vox-e2e` mode + `assert_vox_geometry_visible` non-skybox gate landed; addendum logged in `03a-impl-vox-loading.md`.
- [ ] Step 8c — checkpoint + dispatch `review-vox`
- [ ] Step 7c — synthesis after `review-vox`, hard gate
- [ ] Step 8c — checkpoint + dispatch `impl-editor`
- [ ] Step 7d — synthesis after `impl-editor`, hard gate
- [ ] Step 8d — checkpoint + dispatch `review-editor`
- [ ] Step 7e — final synthesis, exit

## Track order (user-confirmed)

VOX → editor → (GPU algorithmics dropped).

## Deferred (intact, on disk)

The Phase-D residuals from `12-alignment-gap.md` §6 / `14-paper-gap.md` are not
abandoned — durable artifacts live at `docs/orchestrate/phase-d-completion/`
(reuse audit + wgpu-barrier research) for a future circle-back. Q&A answers
preserved in the transcript: W1 diagnostic-first, W2 C#-faithful, three
architect tracks.
