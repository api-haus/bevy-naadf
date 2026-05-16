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
| `03a-followup-empty-scene-diagnosis.md` | Track A follow-up — diagnose + fix empty-scene + camera-dark on real `.vox` file; lifted Decision-6 identity-only walk. | **complete** (general-purpose Opus, 2026-05-15) — committed `44d0599` |
| `02a-v2-sparse-vox-ingestion.md` | Track A redesign — sparse VOX ingestion (supersedes 02a Decision 3); enables Oasis-scale `.vox` loads. | **complete** (delegate-architect, 2026-05-15) |
| `03a-v2-impl-sparse-vox.md` | Track A v2 implementation log — sparse VOX ingestion against `02a-v2`; Oasis_Hard_Cover.vox now loads cleanly at 93×34×84 chunks. | **complete** (general-purpose Opus, 2026-05-15) — uncommitted on disk |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs` | Track A E2E gate addendum — synthesised-fixture `--vox-e2e` mode + `assert_vox_geometry_visible` non-skybox gate. Logged in `03a-impl-vox-loading.md` `## E2E gate addendum`. | **complete** (general-purpose Opus, 2026-05-15) — uncommitted on disk |
| `03b-impl-editor.md` | Track B implementation log — paint/cube/sphere brushes + `KnobKind::Edit` panel extension + top-right tool HUD + `WorldData::ray_traversal`/`set_voxels_batch`/`get_voxel_type`. | **complete** (general-purpose Opus, 2026-05-15) — uncommitted on disk |
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
- [x] **Step 8b-v2-redesign** — checkpoint + dispatch `design-sparse-vox` (user-directed; large-world support — Oasis_Hard_Cover.vox exceeded v1 caps). Architect landed `02a-v2-sparse-vox-ingestion.md`.
- [x] **Step 7b-v2** — synthesis after redesign, hard gate
- [x] **Step 8b-v2-impl** — checkpoint + dispatch `impl-sparse-vox`. Sparse path landed; Oasis_Hard_Cover.vox loads at 93×34×84 chunks (~50 MiB sparse vs ~140 GB dense). All 6 Δ-decisions honored; 151 tests pass; 5 e2e modes PASS.
- [x] **Step 8b-v2-camera-init** — checkpoint + dispatch `impl-camera-init` (user-directed; faithful-port camera-init-on-vox-load). Addendum landed in `03a-v2-impl-sparse-vox.md`; Oasis frames at `(726.56, 850.0, 52.5)` looking +Z; 154 tests pass; 5 e2e modes PASS.
- [x] **Step 7b-v2-impl** — synthesis after impl-sparse-vox + camera-init, hard gate. Committed `cb86e53` (sparse VOX) + `03ce9f0` (camera-init). Track A user-verified end-to-end.
- [x] **Step 8c-impl-editor** — checkpoint + dispatch `impl-editor` (user-confirmed; skipped `review-vox`). Editor sub-tree landed; 170 tests pass; all 5 e2e modes PASS; zero regression on Track A.
- [x] **Step 7d** — synthesis after `impl-editor`, hard gate. User reported 4 bugs from manual editor verification; Bug 1 (async edits) deferred for consideration; Bugs 2/3/4 to be fixed next.
- [ ] **Step 8e-editor-fixes** — checkpoint + dispatch `fix-editor-bugs-234` (user-directed; AADF invalidation on .vox + continuous paint wiring; Bug 1 / async edits deferred separately) (← we are here)
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

### Editor — Bug 1: large edits freeze the app (binding rule, deferred)

**Rule (user-confirmed 2026-05-15):** *all big edits must be async.* Synchronous
`set_voxels_batch` blocks the main thread; a sphere of radius 16 ≈ 17K voxels
in ~125 chunks ≈ ~125ms freeze (decode 2048-u32 window + apply + re-encode +
alloc per chunk). The C# spreads the cost via the 7-round flood-fill BFS but
the per-batch encoding is still synchronous; the port needs a stronger fix.
Architectural shape: spawn `set_voxels_batch` body on `AsyncComputeTaskPool`,
drain into `pending_edits` over frames, brush emits a future + an immediate
"pending" marker. Deferred for separate consideration. **Status: not started.**
