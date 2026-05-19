# 01 — Context: TAA hash world-data identity fix

You are the consolidated agent for this orchestration. This document is the full context. The orchestration is in consolidated mode — you design, self-review, and implement in one uninterrupted run, flushing each stage to `05-impl-taa-hash-world-identity.md` as you go.

## Worktree

All paths below are inside:
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/`

Branch: `feat/streaming-world` (already checked out). Working tree clean as of `cf538e6` (Phase 2.14.g e2e regression sweep — 3/3 gates PASS).

## Goal (verbatim from handoff `/tmp/taa-streaming-hash-handoff.md`)

> The bevy-naadf streaming-world refactor (Phase 2.14, just merged on branch `feat/streaming-world`) made procedural streaming visibly correct: the world loads fully at startup, then streams as the camera walks. The remaining visual artifact: **on every camera reposition that triggers an origin shift, shadowed regions briefly fill with noisy splotches that decay over a few frames.** This is TAA reprojecting against history that was rendered with a different world-segment under the same screen pixel, but the existing hash-reject test in the reproject pass does not catch the swap because the hash only encodes surface-material classification, not world identity.
>
> This is a focused fix, not an investigation.
>
> Mode: Diagnosed.

## Root cause (code-grounded)

The TAA hash-reject test at `crates/bevy_naadf/src/assets/shaders/taa.wgsl` ~line 362–371 (`if (s.hash != valid_hash_center) { ... continue; }`) is the mechanism that should invalidate history when the world data under a pixel changes. The hash function at `crates/bevy_naadf/src/assets/shaders/taa_common.wgsl` ~line 44–56 (`taa_hash_from_data(is_diffuse, specular_normals, entity)`) only mixes **surface-material classification** into the hash and ignores **world-data identity** (segment id / voxel position / data version). For shadowed all-diffuse-no-entity-no-specular regions, both the pre-swap and post-swap samples hash to the same constant value, so the reject doesn't fire and TAA keeps accumulating against stale history.

The shader file header at `taa_common.wgsl:46-48` already predicted this:

> "In Phase A's plane-0-only, entity-free, all-diffuse world this collapses to a single constant value for every hit pixel — but it is ported faithfully (it is cheap and Phase B needs it varying)."

Phase B's "varying-ness" never came from world data — only from `specular_normals` / `entity`. With procedural streaming + voxel edits the world data varies independently of those fields, and the hash misses it.

## Fix — three load-bearing shader sites

All three sites are in `crates/bevy_naadf/src/assets/shaders/`. **Important — the handoff and the audit both used slightly different path prefixes; the correct prefix inside this worktree is `crates/bevy_naadf/src/assets/shaders/`.** Verify with `Read` before editing; the line numbers below were given by the handoff and may have shifted by a small number of lines.

### Site 1 — `taa_common.wgsl` (`taa_hash_from_data`, ~line 44-56)

Current:
```wgsl
fn taa_hash_from_data(is_diffuse: u32, specular_normals: u32, entity: u32) -> u32 {
    var hash = is_diffuse | (entity << 1u) | (specular_normals << 15u);
    hash = hash ^ (hash >> 17u);
    hash = hash * 0xed5ad4bbu;
    hash = hash ^ (hash >> 11u);
    hash = hash * 0xac4c1b51u;
    return hash;
}
```

Bits available in the pre-mix word: bit 0 = `is_diffuse`, bit 1 = `entity` LSB, bit 15 = `specular_normals` LSB. **Bits 2-14 are unused — 13 bits free.**

Extend signature with `data_id_lo13: u32` and OR into bits 2-14:
```wgsl
fn taa_hash_from_data(
    is_diffuse: u32,
    specular_normals: u32,
    entity: u32,
    data_id_lo13: u32,  // NEW — low 13 bits of world-data identity
) -> u32 {
    var hash = is_diffuse
        | (entity << 1u)
        | ((data_id_lo13 & 0x1FFFu) << 2u)  // NEW
        | (specular_normals << 15u);
    hash = hash ^ (hash >> 17u);
    hash = hash * 0xed5ad4bbu;
    hash = hash ^ (hash >> 11u);
    hash = hash * 0xac4c1b51u;
    return hash;
}
```

The 13-bit input space → 8192 distinct identities. After avalanche, the 16-bit masked output retains the variance.

### Site 2 — `taa.wgsl` (`reproject-pass centre + neighbour hash precompute`, ~line 262-269)

Current:
```wgsl
let cur_hash = taa_hash_from_data(
    cur_first_hit_is_diffuse, cur_first_hit_specular_normals, cur_first_hit_entity,
) & 0xFFFFu;
```

The 9-iteration loop above (~line 217 onwards) reads `cur_first_hit` from `cnts_first_hit[...]`. The first-hit struct (`FirstHitResult`, defined at `crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl:69-76` per audit) has fields including `pos: vec3<f32>` (camera-int-relative). The decode site in `taa.wgsl` is around line 216 — read it before editing.

### Site 3 — `taa.wgsl` (`calc_new_taa_sample`, ~line 457-460)

Current:
```wgsl
let sample_comp = taa_compress_sample(
    dist, light, first_hit_result.normal_tang & 0x7u, is_diffuse,
    specular_normals, extra_data8, first_hit.x & 0x3FFFu,
);
```

`taa_compress_sample` (`taa_common.wgsl:80-117`) calls `taa_hash_from_data(is_diffuse, specular_normals, entity)` at line 107. Update its signature to also take `data_id_lo13` and forward it; update call site `taa.wgsl:457` to pass the same `data_id_lo13` derived as in site 2.

The hash written into the TAA sample this frame is what the **next** frame's reproject pass at site 2 will compare against. They MUST use the same derivation for the reject to be consistent.

## User decisions from Q&A (binding, 2026-05-19)

### Decision 1: `data_id_lo13` derivation MUST be world-absolute

The handoff's literal text said "Camera-int relative is fine". The auditor flagged this is wrong: `FirstHitResult.pos` is **camera-int-relative**, NOT world-absolute (`render_pipeline_common.wgsl:375` initialises `r.pos = cam_pos_frac` and accumulates ray-segment offsets; `cam_pos_int` is NOT added back as a whole-number offset).

Consequence: with camera-relative `floor(pos)`, the SAME world voxel produces DIFFERENT 13-bit IDs before and after an origin shift (the two `pos` values differ by `newCam_int - oldCam_int`). The hash reject would then fire on every origin-shifted pixel even when world data hasn't actually changed under that pixel — over-rejection, the opposite-but-also-wrong failure mode.

User decision: **add `vec3<f32>(cam_pos_int)` before `floor` so same world voxel = same hash across origin shifts.**

At Site 2: `cam_pos_int` is available as the local `cam_pos_int = params.cam_pos_int.xyz` (audit gives line 182).
At Site 3: `cam_pos_int = cnts_params.cam_pos_int.xyz` (audit gives line 407).
Both are `vec3<i32>`; cast to `vec3<f32>` before adding. Verify exact variable names with `Read` before using.

Canonical derivation:
```wgsl
let voxel_pos = vec3<i32>(floor(first_hit_result_pos + vec3<f32>(cam_pos_int)));
let pos_id =
      (u32(voxel_pos.x & 0xF))
    | (u32(voxel_pos.y & 0xF) << 4u)
    | (u32(voxel_pos.z & 0xF) << 8u)
    | ((u32(voxel_pos.x >> 4) ^ u32(voxel_pos.y >> 4) ^ u32(voxel_pos.z >> 4)) & 0x1Fu) << 8u;
// Final shape is the impl agent's call — but BOTH SITES MUST USE THE SAME DERIVATION.
```

Note the audit identified the last `<< 8u` likely should be `<< 12u` or similar to avoid colliding with the y/z low-nibble fields — see audit borderline note and pick a packing that gives 13 stable bits with minimal collision across the 512-slot streaming window. Document the chosen packing as a `## Decisions` entry.

### Decision 2: consolidated mode (this dispatch)

Step 2.5 found all four criteria hold (bounded context, single cohesive scope, low blast radius / shader-only / reversible, tight design↔impl coupling). One Opus 1M-context agent, one uninterrupted run.

### Decision 3: ADD a Rust unit test for `taa_hash_from_data`

Port the WGSL `taa_hash_from_data` arithmetic to a Rust helper (in the appropriate test module — likely under `crates/bevy_naadf/src/` next to existing TAA code; locate via `Grep` for existing TAA-related tests). Test: ≥100 distinct `data_id_lo13` inputs (with all other args held constant) produce ≥99 distinct 16-bit-masked outputs (with very high probability under the avalanche; allow a small collision tolerance). Per global memory `feedback-primitives-then-analytical-invariants.md`.

## Required reading (in order)

1. **THIS file** in full (you are reading it).
2. **`00-reuse-audit.md`** in the same directory — concrete `FirstHitResult` field layout, the `cam_pos_int` correction reasoning, the list of rejected reuse paths (don't re-propose them).
3. **`/tmp/taa-streaming-hash-handoff.md`** — the verbatim handoff source-of-truth (also includes verification commands and forbidden moves).
4. **`crates/bevy_naadf/src/assets/shaders/taa_common.wgsl`** — full file.
5. **`crates/bevy_naadf/src/assets/shaders/taa.wgsl`** — lines 180-470 specifically (cam_pos_int locals, reproject loop, `calc_new_taa_sample`).
6. **`crates/bevy_naadf/src/assets/shaders/render_pipeline_common.wgsl`** — lines 69-76 (`FirstHitResult` struct) and lines 360-410 (`get_hit_data_from_planes` to confirm `pos` frame-of-reference).
7. **`CLAUDE.md`** at the worktree root — verification discipline (no `cargo run --bin bevy-naadf`, only build + `--lib` test + named e2e gates).

## Required reading from project memory (for the impl agent's behaviour rules)

- `feedback-e2e-must-drive-actual-main.md` — any CLI flag you add must be on the shared `AppArgs`/clap parser, not e2e-only. **NOTE for this task**: you are NOT expected to add any new CLI flag. Hash is automatic. If you discover a need to add a flag, escalate via `## Independent review` rather than improvising.
- `feedback-e2e-gates-must-fail-fast.md` — wrap all e2e gates in `timeout 180s`. Verification block below already does this.
- `feedback-primitives-then-analytical-invariants.md` — test primitives first; the Rust unit test is mandated by user Q&A decision 3.
- `subagent-gpu-app-verification-loop.md` — **one smoke per command**. If a gate fails: read failure, fix ONCE, re-run ONCE. If still failing, STOP and report — do NOT loop rebuild→rerun.

## Verification (mandatory)

In order, each invoked ONCE, each wrapped in `timeout`. Wall-clock from the handoff:

1. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo build --workspace 2>&1 | tail -100`
2. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 300s cargo test --workspace --lib 2>&1 | tail -60`
   — expect ≥ 289 passing (post-Phase-2.14.f baseline). The new Rust unit test should be in this count.
3. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate streaming-cold-start 2>&1 | tail -80`
4. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate streaming-window 2>&1 | tail -80`
5. `cd /mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world && timeout 180s cargo run --release --bin e2e_render -- --gate oasis-edit-visual 2>&1 | tail -80`

**Stretch goal** — after fix passes, try tightening the `oasis-edit-visual` Δ-floor threshold (currently 8.00, achieving 17.93–18.01 — the slack was partly because TAA was holding pre-edit history at edit sites, the bug being fixed). Try a tighter floor; if it doesn't move meaningfully, leave it and note the investigation in `## Implementation log`.

**Strictly forbidden as verification** (per worktree `CLAUDE.md`):
- `cargo run --bin bevy-naadf` (any args). Boots a windowed app; proves nothing. Live visual check is the user's job.
- `cargo run --release --bin bevy-naadf` smokes of any kind.

## Forbidden moves (binding)

- No new auxiliary buffers, no new bind-group entries. The hash already has a write path (`taa_compress_sample`) and a read path (the reproject loop). Only the input space changes.
- No changes to streaming code (`crates/bevy_naadf/src/streaming/**`). This is a TAA-pipeline-only fix.
- No ranked hypothesis lists. The diagnosis is single-cause; act on it.
- No "let me investigate first" — read the three sites, read ~20 surrounding lines, then implement.
- No new CLI flags (the fix is automatic). If you discover a need, escalate to `## Independent review` rather than adding one.
- No commits. The orchestrator will commit at the next phase boundary.
- Self-review for high-risk findings only: if you decide any change has a high blast radius, escalate to `## Independent review` with a recommendation to dispatch a fresh-eyes `delegate-reviewer`, do NOT self-certify.

## Deliverable (your output)

Write everything to:
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/streaming-world/docs/orchestrate/taa-hash-world-identity/05-impl-taa-hash-world-identity.md`

Required sections (write in order, append to the file as you progress):

1. `## Design` — short. The chosen `data_id_lo13` packing (with bit-layout diagram), the chosen Rust unit test shape, files-and-functions touched.
2. `## Decisions & rejected alternatives` — explicit list. At minimum: why world-absolute over camera-relative (rationale from this context); whether to use `pcg_hash` as a pre-mixer (audit borderline note); the exact bit-packing chosen and any collisions accepted.
3. `## Assumptions made` — what you took for granted. The implementer who comes after you (if there is one) must be able to spot every implicit decision.
4. `## Independent review` — self-review. Be adversarial about your own design. Anything you rate high-risk MUST recommend a follow-up fresh-eyes `delegate-reviewer` dispatch rather than self-certify.
5. `## Diffs landed` — file:line list of all edits, plus one-line per-diff intent. (No pasted diffs; just refs.)
6. `## Verification` — pass/fail for each of the 5 commands above; tail-snippet of failures only if any.
7. `## Stretch result` — `oasis-edit-visual` threshold tightenability: yes/no/by how much. Honest "I tried X, threshold went from 8.00 to Y, kept/reverted" — no need to land tightening if it doesn't move.
8. `## Out-of-scope findings` — anything you noticed but didn't fix.

After writing, also append a one-line update to the README phase tracker — Phase B → [x], Phase C → status.

## Status return (after writing)

Return only:
- Path of the impl log written
- Pass/fail summary line for each verification command
- Whether anything was escalated to a fresh-eyes reviewer
- Stretch threshold result

Do NOT paste diffs or design text in your return. The orchestrator will read the file.
