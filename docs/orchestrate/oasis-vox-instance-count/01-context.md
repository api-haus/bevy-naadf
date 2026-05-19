# 01-context — Oasis .vox instance-count parity

## Goal (verbatim from user)

> "our version loads only 2.5 modulo-wrapped instances of Oasis .vox, whereas c# version loads 4 -EXACTLY"

Fix the Bevy port so it loads **exactly 4** modulo-wrapped instances of the Oasis `.vox` asset, matching the C#/MonoGame NAADF reference behaviour exactly.

Issue #2 (rays terminating too early at grazing angles, limiting view distance) is held for a **separate** `/delegate` invocation. **Do not address issue #2 in this work.**

## User-supplied hypothesis (priors, not constraints)

The user speculates the root cause is **world extent / scene size** — i.e. the Bevy world AABB or scene-extent constant is smaller than the C# one, so the modulo-wrapped Oasis tiles fewer times before the world ends. This is a **starting hypothesis only**. Verify against actual code; do not assume.

## User-supplied constraint — SINGLE SOURCE OF TRUTH (load-bearing)

User constraint, verbatim:

> "if we have DIFFERENT hard-coded scene-sizez constants - rewritten many times AS RESULT OF AGENTIC DEVELOPMENT WITH LIMITED CONTEXT WINDOW, whereas c# has A SINGULAR ONE - WE HAVE TO REFACTOR"

This means the **audit's deliverable is not just "find the one constant that fixes the count"** — the audit MUST enumerate **every** hard-coded scene-size / world-extent / Oasis-placement / wrap-modulo constant in the Bevy codebase. If multiple divergent values exist, the design phase MUST refactor them to a single source of truth that matches C#'s singular constant. If only one exists, a minimal-diff value change is sufficient.

The audit's reuse table MUST include a column or section listing every such constant found, its file:line, and its current value, so the architect can decide between "patch the value" and "refactor to SSoT".

## Codebases

- **Target (Bevy port — what we fix):** `/mnt/archive4/DEV/bevy-naadf/`
  - Workspace root with `crates/bevy_naadf/`
  - Shaders: `crates/bevy_naadf/src/assets/shaders/`
  - E2E gates: `crates/bevy_naadf/src/e2e/` (vox-related: `pbr_visual.rs`, `pbr_hard_edge.rs`, `pbr_debug_modes.rs`, plus existing `--vox-e2e` and `--oasis-edit-visual` modes in `e2e_render` binary)
- **Reference (C# NAADF — what we match):** `/mnt/archive4/DEV/NAADF/NAADF/`
  - Library/engine: `Libraries/VoxelsCore/`, `World/Data/`, `World/Generator/`, `World/Render/`
  - HLSL shaders (mirror of WGSL pipeline): `Content/shaders/render/**`

## Required reading (in order, for non-review agents)

1. This file (`01-context.md`)
2. Agent's own group file (varies)
3. Prior agents' `## Decisions & rejected alternatives` and `## Assumptions made` sections (design/impl agents only)
4. **Project rules:** `/mnt/archive4/DEV/bevy-naadf/CLAUDE.md` — note the "verification discipline" section: **agents must NOT run `cargo run --bin bevy-naadf` as a verification step.** Use the deterministic gates listed there if a programmatic check is needed.
5. **Phase context (port history):** `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/naadf-bevy-port/01-context.md` — the original orchestration scope, port philosophy, faithful-port rule.

## Faithful-port rule (binding)

Quoted from auto-memory `bevy-naadf-faithful-port-rule.md`:
> No Bevy-only microoptimizations or behaviors not in C# NAADF; default = match C#, even when C# has the bug. Deliberate divergences require explicit user approval + docs entry.

This means: **the fix is whatever brings Bevy to C# parity**. If C# uses an arbitrary magic number `N`, Bevy uses `N`. Do not "improve" or generalise beyond C#.

## Forbidden moves

- **Do not** run `cargo run --bin bevy-naadf` to "verify" the fix. The user does the visual check.
- **Do not** address grazing-angle ray-termination (issue #2). Scope discipline.
- **Do not** add a new e2e gate for this fix (user chose "user-eyes only" verification in the Q&A).
- **Do not** start the design phase without knowing whether one or multiple divergent constants exist — that determines fix-vs-refactor.

## Decisions from Q&A

| Q | Decision | Notes |
|---|---|---|
| Q1 (scoping) | User speculates **world extent / scene size** is the wrap surface | Hypothesis FALSIFIED by Phase-1 audit. Root cause is asset-level (Bevy loads `oasis_hard_cover.vox` 1488×544×1344; C# loads `oasis.cvox` 1033×386×1082). World-size constants in Bevy already match C# exactly. |
| Q2 (success gate) | **User-eyes only** | No new e2e gate. User does side-by-side visual check vs C#. |
| Q3 (fix scope) | **Minimal — just match C#** | SSoT-refactor constraint does NOT trigger: audit found only one canonical world-size constant chain, already matching C#. |
| Q4 (fix direction, **post-audit, given by user verbatim**) | **"alright, lets implement both vox and cvox parsers and extend the drag&drop and autoload functionality to support both based on parsed header magic"** | The Bevy port gains a faithful `.cvox` parser (port of C# `ModelData.Load` at `/mnt/archive4/DEV/NAADF/NAADF/World/Model/ModelData.cs:181-258`). Existing `.vox` parser stays. A new dispatch entry point sniffs the magic bytes of the input file and routes to the appropriate parser. Both drag-and-drop and autoload paths consume the dispatch entry point, not the per-format parsers. Once landed, the user can drop `oasis.cvox` (copied from `/mnt/archive4/DEV/NAADF/NAADF/Content/oasis.cvox`) into the Bevy port and observe the expected 4 modulo-wrapped instances. |

## Output shape contract for the audit

The `delegate-auditor` agent's deliverable must include:

1. **The Oasis .vox load + placement path** in Bevy (file:line for the loader, the scene-setup, the wrap-modulo / repeat logic in either Rust scene code or WGSL shader).
2. **All hard-coded scene-size / world-extent / Oasis-placement / wrap-modulo constants** in the Bevy codebase. One row per constant: `file:line`, current value, where it's read, what it controls. This is the load-bearing list that decides fix-vs-refactor.
3. **Borderline candidates** — anything that *might* affect the instance count but the auditor isn't sure (so the architect can decide).

The C# reference scan must produce the **mirror** of (1) — the equivalent .vox / Oasis / world-extent path in C# — and explicitly call out the **single canonical constant** (if it is indeed singular).
