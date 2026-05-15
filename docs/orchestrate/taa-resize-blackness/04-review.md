# 04 — Review brief: taa-resize-blackness

> **REVIEWER:** Read ONLY this file. Do NOT read `01-context.md`, `02-design.md`, or any prior implementer log. You are deliberately given criteria + artifact pointers only — the design rationale is withheld so you can catch assumptions the design baked in. Fresh-eyes generator–verifier separation.

---

## What was built
The change set delivered under topic `taa-resize-blackness` adds a failing reproduction test for a TAA / GI ring-drain bug, then applies a fix that keeps the test passing. Two implementer phases:

- **Impl-A** (`03a-impl-test.md`) — adds a failing reproduction test inside the e2e harness.
- **Impl-B** (`03b-impl-fix.md`) — applies the fix; the same test now passes.

## Artifact to review
- The full diff against `main` HEAD at the time of this orchestration (use `git diff main...HEAD` or the equivalent for whichever branch the work landed on).
- `docs/orchestrate/taa-resize-blackness/03a-impl-test.md` and `docs/orchestrate/taa-resize-blackness/03b-impl-fix.md` — implementer logs, for verifying that what was promised matches what was done.

## Success criteria

### Functional
1. **The test reliably reproduces the bug.** Running the test on `main` (before the fix) reports a luminance-collapse failure in the post-resize shadow-band region. Running it after the fix reports a pass.
2. **The fix actually solves the bug, not the symptom.** Verify the fix addresses the *cause* (TAA + GI ring zero-clears on `pixel_count` change in `crates/bevy_naadf/src/render/taa.rs` and `gi.rs`), not just suppresses the assertion threshold or skips the assertion in some condition.
3. **Pre-existing fix #4** in `crates/bevy_naadf/src/render/extract.rs` (the last-known-good viewport retain — `extract.rs:121-165`) is **not modified**. It guards a different case (bogus-1×1 degenerate frame) and is correct as-is.
4. **No new headless or `#[test]` GPU paths.** The repro test must run inside the existing `e2e_render` binary harness. `cargo test --lib` still has zero GPU-driven tests.
5. **The fix does not break any existing e2e batch.** All prior `assert_batch_N` gates still pass.

### Code-quality
6. **No new screenshot baselines / hash gates introduced.** Reuse existing luminance-region gates only.
7. **No expansion of scope into `TaaGpu.camera_history` rebuild** unless the implementer log explicitly justifies it as load-bearing for the test to pass. If it was modified, flag for explicit confirmation.
8. **No regressions in the e2e driver state machine** — the new phase (likely `E2ePhase::Resize`) is well-bounded, has a finite frame budget, and exits cleanly.
9. **Conventional Rust style** — error handling matches surrounding code, no `unwrap()` in render-world systems where the surrounding code uses `let Some(x) = ... else { return; }`, no new `pub` items in modules that were previously crate-private without justification.

### Determinism / lifecycle
10. **The fix preserves bit-exact behaviour on the no-resize path.** A run with no resize triggers must produce the same output as before the fix.
11. **No new resources / events / systems with ambiguous schedule placement.** Any new system has explicit `before` / `after` constraints if its ordering matters.

## Review deliverable
Write your review to `docs/orchestrate/taa-resize-blackness/05-review.md` under the heading `## delegate-reviewer findings (<ISO date>)`. Structure:

```markdown
## Verdict
PASS | PASS-WITH-NITS | FAIL

## Per-criterion findings
1. [PASS/FAIL/UNCLEAR] — <evidence: file:line or test output>
2. ...

## Code-level concerns
- <bullet — file:line — what + why>

## Questions for the orchestrator
- <anything you couldn't verify from the diff alone>
```

Verify the file is on disk before returning. Return only a status summary — do not paste the review back as agent return text.
