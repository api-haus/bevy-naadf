# PBR raymarching — orchestration

Extend the NAADF voxel raymarcher with **unified PBR shading**: triplanar
texture-array sampling (albedo+AO / normal / metallic+roughness+height /
emissive), parallax-occlusion-mapping at the voxel face, energy-conserving
metallic/dielectric BRDF, and glossy reflections — reusing the existing
VNDF-GGX BRDF and multi-bounce `shoot_ray` scaffolding already in the shaders.

## Files

| File | Owner | Status |
|---|---|---|
| `01-context.md` | orchestrator | written |
| `00-reuse-audit.md` | `delegate-auditor` × 2 | written |
| `02-design.md` | `delegate-architect` | pending |
| `03-impl.md` | implementer (`general-purpose`, Opus) | pending |
| `04-review.md` | orchestrator (brief) → `delegate-reviewer` (findings) | pending |

## Agent groups

- **audit** — read-only reuse audit of existing raymarcher, BRDF, voxel
  material, texture-array builder, baker pipeline. `delegate-auditor` on Sonnet.
  Output appended to `00-reuse-audit.md`.
- **setup** — mechanical extraction of 7 CC0 texture zips into the worktree's
  `assets/materials/<name>/`. One-shot `general-purpose` Sonnet sub-agent.
- **design** — `delegate-architect` on inherited Opus. Reads `01-context.md` +
  `00-reuse-audit.md`. Designs: VoxelType reshape (drop `roughness`,
  `material_base` enum, `color_layered`, `color_base`; add
  `material_layer_index: u16`, `albedo_tint: Vec3`, `is_emissive: bool`);
  GpuVoxelType bit packing; MaterialSet asset/resource; the 4 linked
  `.texarray.ron` re-author plan (existing pipeline already supports it);
  triplanar + POM WGSL design; unified BRDF (single PBR branch + emissive
  fast-path); new e2e gate spec. Writes to `02-design.md`.
- **impl** — `general-purpose` Opus. Reads `01-context.md` + `02-design.md`
  (plus the architect's `## Decisions & rejected alternatives` and
  `## Assumptions made` sub-sections). Implements WGSL, Rust, bind groups,
  baker reauthor, e2e gate. Runs project verification gates between steps.
  Writes step-by-step log to `03-impl.md`.
- **review** — `delegate-reviewer` on inherited Opus. Reads ONLY `04-review.md`
  (success criteria + artifact pointer). Fresh-eyes; orchestrator reconciles
  flags against full context at Step 7 synthesis.

## Phase checklist

- [x] Audit dispatched
- [x] Audit follow-up (baker pipeline) dispatched
- [x] Q&A complete (4 questions + 1 emissive follow-up + post-pivot 3 questions)
- [x] User pivot: ALL surfaces are PBR; metallic from texture; 4 linked
      texture arrays per material
- [x] User input: 7 CC0 PBR texture sets supplied
- [x] `01-context.md` written
- [x] `README.md` written
- [x] `04-review.md` skeleton (criteria only)
- [x] Checkpoint commit (audit + context files) — `ddd092f`
- [x] Texture extraction dispatched (7 materials + 3 placeholders)
- [x] Hard gate — extraction reviewed
- [x] Checkpoint commit (extracted assets) — `7fb962e`
- [x] `delegate-architect` dispatched
- [x] Hard gate — design reviewed
- [x] Checkpoint commit (design) — `85105f3`
- [x] Implementer dispatched (Stage 8 SUCCESS — all 9 gates pass)
- [ ] Hard gate — impl reviewed (user does live visual check on the binary)
- [ ] Checkpoint commit (impl)
- [ ] `delegate-reviewer` dispatched
- [ ] Final synthesis + user sign-off

## Worktree

All work happens in:
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/pbr-raymarching`

Branch: `feat/pbr-raymarching`.
