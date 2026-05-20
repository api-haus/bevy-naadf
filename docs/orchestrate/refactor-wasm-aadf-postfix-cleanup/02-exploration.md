# 02-exploration — refactor-wasm-aadf-postfix-cleanup

Empty stub. The `refactor-explorer` agent writes its findings here under
the heading `## refactor-explorer findings (<ISO date>)`.

## refactor-explorer findings (2026-05-20)

### Findings summary table

| # | Severity | Location | Smell type | One-line description |
|---|----------|----------|------------|----------------------|
| 1A | high | `bounds_calc.rs:538-553` | docblock-rot / iter-N narrative | 16-line block narrates iter-1 / iter-2-2 / iter-2-3 brute-force history; describes a future-tense probe ("iter-2-3 if needed") not the current behavior. |
| 1B | high | `bounds_calc.rs:93-105` + `mod.rs:161-168` + `bounds_calc.wgsl:117-125` | docblock-vs-code contradiction | Three docblocks claim native "never accesses" `chunks_mirror`; WGSL unconditionally reads from `chunks_mirror` on both targets at `bounds_calc.wgsl:273` and `:523`, and Rust unconditionally issues `copy_buffer_to_buffer` at `bounds_calc.rs:635-668` (no `#[cfg]` gate). Code is correct; docblocks lie. |
| 1C | medium | `bounds_calc.wgsl:123-124` | stale line refs | Header comment cites "line 499 own-AADF read", "line 252 neighbour read", "write at line 538" — actual lines are 523, 273, 564. All three off. |
| 1D | medium | `bounds_calc.rs:527-534` + `:670-674` + WGSL `:463-467` + WGSL `:432-462` + `mod.rs:546-555,659-664,750` | resolved-but-undispatched pipeline (dead code) | `end_of_encoder_noop_pipeline` is queued, cached, resolved per-frame on wasm, gated by `let Some(...) else { return; }`, and then immediately `let _ = ...`'d to silence the unused-variable warning. Never dispatched. Same shape as the `chunks_scratch` Locals already removed in `c6b0deb`. |
| 1E | medium | `bounds_calc.rs:556-570` | one-shot diagnostic embedded mid-function | `[aadf-probe]` static `AtomicBool` one-shot log block (15 lines, 4 lines of payload) lives inline in the hot path; orthogonal to dispatch logic; verifies a config value that has its own const docblock. |
| 1F | medium | `bounds_calc.rs:593-668` | mixed concerns: copy-seed + dispatch-loop + per-round-copy interleaved | The function body intermingles a one-shot seed copy (627-640), the per-round dispatch (642-655), and the between-rounds copy (657-667), inside one `naadf_bounds_compute_node` body, with iter-2 history commentary attached to each block. Natural extract-helper boundary obscured by the narrative. |
| 1G | low | `bounds_calc.rs:459-468` | repetitive `cfg_attr(not(wasm), allow(unused_variables))` annotations | Four parameters (`render_device`, `render_queue`, `world_gpu`) each carry their own `#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]` decoration even though all of them are now USED on both targets (see finding 1B: native unconditionally copies). |
| 2A | high | `bounds_calc.wgsl:564` (write) vs `:273` + `:523` (reads) | cross-workgroup write-via-non-atomic, read-via-different-buffer | Writes to `chunks[chunk_idx]` from one workgroup, reads from `chunks_mirror[neighbour_idx]` from another workgroup (line 273, neighbour read) in the SAME pass — but `chunks_mirror` is only refreshed BETWEEN passes via `copy_buffer_to_buffer`. The neighbour-read at line 273 reads a chunk that a DIFFERENT workgroup in this round may be writing concurrently to `chunks` (visible only after next round's copy). This is the structural cause of the ~18% web parity gap. |
| 2B | medium | `bounds_calc.wgsl:273` + `:523` | dual-purpose mirror (cross-frame + intra-pass) | `chunks_mirror` is described as cross-frame propagation (between encoder submits), but the algorithm ALSO uses it for in-round neighbour reads (which is intra-pass on native — neighbour writes go to `chunks`, neighbour reads come from `chunks_mirror`). Conflating "cross-frame buffer" with "this-pass read shadow" muddies the mental model. |
| 2C | low | `bounds_calc.wgsl:113-114` + `:443-451` | mixed-view of same backing buffer | `chunks` is declared `array<vec2<u32>>` (read_write, non-atomic) in `bounds_calc.wgsl`, but the same backing GPU buffer is bound in other shaders (`chunk_calc.wgsl`, `world_data.wgsl`). Iter-3's `array<atomic<u32>>` view of the SAME buffer was reverted in `960eeb2` (resolving prior flag from brute-force side-note 3); however the multi-shader-different-view fragility persists. |
| 3A | low | `tests.rs:529` + `:533` | test-vs-production-const drift | Test allocates `2048 * 16 = 32 KiB` probe history buffer + `2048 * 4` zeros vec; production const `PREPARE_PROBE_HISTORY_ENTRIES = 256` (`mod.rs:340`). Comment at `tests.rs:526` claims "matches production capacity" — false post-`c6b0deb`. |

### Item 1 — `naadf_bounds_compute_node` cleanup findings

#### Finding 1A: 16-line iter-N narrative block describes a future-tense probe, not current behavior (severity: high)

- **Location:** `crates/bevy_naadf/src/render/construction/bounds_calc.rs:538-553`
- **Current state:** A 16-line block comment headed "2026-05-20 dispatch-2 iter-2-2 H1" that:
  - Justifies "use Bevy's main encoder for wasm W3, same as native" (line 538-540) — but this is the CURRENT, unconditional behavior at line 600 (`let encoder = render_context.command_encoder();`), no longer a deviation worth narrating.
  - References "dispatch-1 iter-0 through iter-5" (line 540) and "the per-round-encoder+submit pattern" — historical hypotheses that were refuted; the function no longer takes that path.
  - Closes with "iter-2-3 if needed will probe whether that bug still exists" (line 552) — future tense pointing to a probe that never happened. The Dawn STORAGE→INDIRECT barrier bug is now mitigated by `compute_workgroups_override` (the direct-dispatch path) lower down; the block doesn't say that.
- **Why this is a problem:** A new reader following the source has to reverse-engineer the resolution timeline from this brute-force-iteration narrative. The function's load-bearing facts (n=1 + chunks_mirror per-frame copy + direct-dispatch override on wasm) are scattered across three separate brute-force-iter commentary blocks (this one + `:572-584` + `:593-598`) instead of stated once at the top.
- **Suggested direction:** Collapse the three iter-N blocks into a single coherent function-level docblock at `:452` (above `pub fn naadf_bounds_compute_node`) that explains the post-fix mechanism in present tense: "On wasm, regime-2 runs ONE compute pass per frame; between frames a `copy_buffer_to_buffer(chunks → chunks_mirror)` provides cross-frame visibility via the TRANSFER-stage barrier. On native, the same code path runs unchanged with native's `n_bounds_rounds = 5` (the wasm clamp lives in `From<&AppArgs>`). The direct-dispatch override exists because Dawn's STORAGE→INDIRECT barrier was empirically broken — orthogonal to the chunks-RMW story." Then remove the three intermediate iter-N narratives entirely.
- **Restructure tier:** comments-only.

#### Finding 1B: chunks_mirror "native never accesses" claim contradicted by code (severity: high)

- **Location:** Three places repeat the same false claim:
  - `crates/bevy_naadf/src/render/construction/bounds_calc.rs:100-104` — "On native this mirror is allocated to satisfy the layout but never accessed (the shader's read path is gated by a #ifdef-WGSL-define equivalent that the Rust side cannot express; instead we ALWAYS access chunks_mirror on both targets and the native code path also issues the copy)."
  - `crates/bevy_naadf/src/render/construction/mod.rs:164-167` — "Native uses chunks directly via the rw binding (writes only) so this buffer is bound but never read on native".
  - `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:118` — "On wasm it is refreshed via `copy_buffer_to_buffer(chunks, chunks_mirror, full_size)` between each W3 round" (implies native does not).
- **Why this is a problem:** WGSL unconditionally reads `chunks_mirror` at `bounds_calc.wgsl:273` and `:523` (no `#define`, no `#ifdef`, no shader-side fork). Rust unconditionally issues `copy_buffer_to_buffer(chunks, chunks_mirror, ...)` at `bounds_calc.rs:635-640` (initial seed) and `:660-667` (per-round) — no `#[cfg(target_arch = "wasm32")]` gates either path. The first docblock partially self-corrects mid-sentence ("instead we ALWAYS access chunks_mirror on both targets") but then leaves the misleading "never accessed" lead in place. A new reader scanning the WGSL would conclude native uses a different code path; the truth is native and wasm run identical code that ONLY differs in `n_bounds_rounds`. Comments lie about behavior — exactly the failure mode that bit the prior orchestration (per `13-minimal-fix-verify.md:121` — orchestrator note on overly-clever framing).
- **Suggested direction:** Reword all three docblocks to state plainly: "`chunks_mirror` is a RO mirror of `chunks`, refreshed via `copy_buffer_to_buffer(chunks, chunks_mirror)` once before the first round and between every subsequent round. ALL `compute_group_bounds` reads of chunk-AADF state (own + neighbour) come from `chunks_mirror`; the only writer is `chunks` (non-atomic rw view). Both targets run the same code; only `n_bounds_rounds` differs (5 native, 1 wasm). On native the copy is also free correctness (chunks_mirror == chunks after the copy)."
- **Restructure tier:** comments-only.

#### Finding 1C: WGSL header line refs are stale post-probe-1B/iter-2-3 churn (severity: medium)

- **Location:** `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:123-124`
- **Current state:** Header comment block at lines 117-125 cites:
  - "line 499 own-AADF read" — actual location: line 523 (`let cur_chunk_full = chunks_mirror[chunk_idx];`).
  - "line 252 neighbour read" — actual location: line 273 (`let neighbour_x = chunks_mirror[neighbour_idx].x;`).
  - "write at line 538" — actual location: line 564 (`chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y);`). Line 538 is mid-mask-setup, not a write site at all.
- **Why this is a problem:** The line drift accumulated over probe-1B and iter-3-revert edits. A reader using these line refs to navigate hits the wrong code; subtle waste of attention. The brief explicitly flags that prior agents made line-ref errors; this is a symptom of the same.
- **Suggested direction:** Either delete the inline line refs (the comment's narrative is enough — "neighbour read in `add_bounds_group`", "own-AADF read in `compute_group_bounds`") or anchor them with a search token (`grep "chunks_mirror\["` would find both sites). Eliminate the bare line numbers — they will drift again.
- **Restructure tier:** comments-only.

#### Finding 1D: `end_of_encoder_noop_pipeline` is resolved-but-undispatched dead code (severity: medium)

- **Location:**
  - WGSL entry point: `bounds_calc.wgsl:432-467` (35 lines of probe-2 narrative + 4-line body).
  - Rust pipeline queue: `bounds_calc.rs:301-329` (`queue_end_of_encoder_noop_pipeline` + `_with_handle`).
  - Resolution site: `bounds_calc.rs:527-534` (wasm-only `let Some(...) else { return; };` — node bails entire frame if pipeline not yet ready).
  - Sink: `bounds_calc.rs:670-674` (`let _ = end_of_encoder_noop_pipeline;` — explicitly silences unused-variable warning).
  - Pipeline storage: `mod.rs:546-555`, queue call `mod.rs:659-664`, struct field `:750`.
- **Current state:** The probe-2 (`M1 confirmation probe`) WGSL entry point was authored to dispatch a no-op atomic load+store at end of each encoder to register `bound_queue_sizes` as the next user with Dawn's PassResourceUsageTracker. The brute-force iteration found it ineffective (per `12-brute-force-summary.md` — neither HM/HN nor the per-round encoder pattern lifted SSIM). The Rust side stopped dispatching it but kept everything else: WGSL entry point, pipeline queueing, pipeline resolution gate (which can BLOCK the whole node from running until the pipeline compiles!), and an explicit `let _ = ...` to silence the unused-variable warning. This is shaped exactly like the `chunks_scratch` / `chunks_scratch_for_fence` Locals already removed in `c6b0deb` — identical "dead-from-iter-N intervention" smell.
- **Why this is a problem:** Three concrete costs: (a) the wasm code path at `:527-534` makes the entire regime-2 node skip whole frames waiting for a pipeline that will never be dispatched; (b) the 35-line WGSL block (`:432-467`) narrates a hypothesis that was refuted; (c) the explicit `let _ = ...` sink is a textbook code smell — a value held only to satisfy the compiler. Note: the brief's "FORBIDDEN MOVES" list at `01-context.md:144-146` says NOT to modify the `[probe1-call]`/`[cpu-gpu-parity]`/`[aadf-probe]`/`[device-snapshot]` instrumentation — but does NOT mention `end_of_encoder_noop` / probe-2 by name. Removing it should be in-scope.
- **Suggested direction:** Architect should decide whether to (a) delete the entry point + pipeline + storage entirely (clean tree, removes one of the cross-pass-visibility probes that may still be diagnostically useful), or (b) keep it dispatched as a `let _ =` no-op AND remove the early-return gate at `:527-534` (preserve as latent probe). My recommendation: option (a). The diagnostic story is done; the probe-2 file lives in the orchestration doc trail. Estimated removal: ~60 lines across 3 files.
- **Restructure tier:** control-flow-tightening (removes one cfg-branch and the early-return guard).

#### Finding 1E: one-shot `[aadf-probe]` config log embedded mid-function (severity: medium)

- **Location:** `crates/bevy_naadf/src/render/construction/bounds_calc.rs:556-570`
- **Current state:** A static `AtomicBool` one-shot guard wrapping a single `bevy::log::info!` that prints `n_bounds_rounds` + `max_group_bound_dispatch`. The values it logs are deterministic per-build (the wasm clamp lives at `config.rs:From<&AppArgs>`); the log fires once per process startup.
- **Why this is a problem:** This is the only `[aadf-probe]` line inside `naadf_bounds_compute_node`. The function is meant to be the regime-2 dispatch loop; this one-shot log of build-config values is orthogonal. It also has a non-obvious cost: the static `AtomicBool` is shared across all node invocations (correct here, but a footgun if a future maintainer ever instantiates two regime-2 nodes for a multi-world scenario). The forbidden-moves list at `01-context.md:144-146` includes `[aadf-probe]` — but the spirit of that protection is the *load-bearing* probes (`[probe1-call]`, `[cpu-gpu-parity]`); this one-shot config dump arguably falls outside it.
- **Suggested direction:** Architect must decide whether `[aadf-probe]`'s blanket-protection extends to this one-shot. If yes: leave it. If the architect judges it expendable: move it to a startup system that runs once at `ConstructionConfig` resource creation — out of the hot path entirely. Either way: not "small + obvious + low-risk" per the brief's commit policy, because moving it touches a probe-instrumentation channel the user pinned.
- **Restructure tier:** extract-helper / startup-system-move (escapes the in-scope restructure latitude if the architect chooses to move it).

#### Finding 1F: interleaved seed-copy / dispatch-loop / per-round-copy with comments-between-steps (severity: medium)

- **Location:** `crates/bevy_naadf/src/render/construction/bounds_calc.rs:593-668` (~76 lines after the iter-N narrative finally ends).
- **Current state:** Three logical phases interleaved with multi-line history commentary:
  1. Lines 593-598: 6-line "what we removed in iter-1" comment (chunks-self-copy reverted, write_buffer scratch reverted).
  2. Line 600: `let encoder = render_context.command_encoder();` + line 601: timespan.
  3. Lines 603-625: 23-line comment narrating iter-2 design + chunks-buffer extraction.
  4. Lines 627-640: initial-seed `copy_buffer_to_buffer(chunks, chunks_mirror)` (gated by `if let (Some, Some)`).
  5. Lines 642-668: the `for round_idx in 0..n_rounds` loop containing dispatch + conditional between-round copy.
- **Why this is a problem:** The reader has to manually re-thread the three phases past the iter-N commentary to see the structure. The Phase-4 initial-seed `copy_buffer_to_buffer` is structurally similar to the per-round copy at Phase 5 — same source/dest, same `min(chunks.size(), chunks_mirror.size())` clamp — but they're duplicated rather than helped out. The destructuring `if let (Some(chunks), Some(chunks_mirror)) = (chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref())` appears twice (lines 635-636 and 661-662) with identical body shape.
- **Suggested direction:** Extract a small local closure (or free function) `refresh_chunks_mirror(encoder, &chunks_buf_opt, &chunks_mirror_buf_opt)` that owns the destructure + copy + size-clamp. Call it once before the loop (seed) and once between rounds (refresh). Loop body collapses to "dispatch round; if not last, refresh mirror." Comment narrative consolidated into the function-level docblock per finding 1A.
- **Restructure tier:** extract-helper.

#### Finding 1G: `cfg_attr(not(wasm), allow(unused_variables))` on parameters that are no longer wasm-only (severity: low)

- **Location:** `crates/bevy_naadf/src/render/construction/bounds_calc.rs:459-468`
- **Current state:** Four function parameters (`render_device`, `render_queue`, `world_gpu`) each carry their own `#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]` annotation:
  ```
  #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
  render_device: Res<bevy::render::renderer::RenderDevice>,
  #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
  render_queue: Res<bevy::render::renderer::RenderQueue>,
  ...
  #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
  world_gpu: Option<Res<crate::render::prepare::WorldGpu>>,
  ```
  But `world_gpu` IS used unconditionally at line 622-624 (`world_gpu.as_ref().map(|w| w.chunks_buffer.clone())`) since iter-2 went to both-targets. `render_device` and `render_queue` are not actually referenced anywhere in the function body (confirmed by reading 452-677).
- **Why this is a problem:** Two distinct issues: (a) the annotation on `world_gpu` is now wrong — it's used on native too; the annotation suppresses a warning that wouldn't fire. (b) `render_device` and `render_queue` are truly unused on both targets — they should be removed from the signature, not annotated. The iter-1 (HM/HN) host-side `queue.write_buffer` fence experiment was the only consumer; that was reverted in commit `a426441` per `12-brute-force-summary.md:96-98` ("iter-1 (HM/HN) host-side write_buffer fence scratch buffer was also reverted as part of the minimal-fix landing").
- **Suggested direction:** Remove `render_device: Res<RenderDevice>` and `render_queue: Res<RenderQueue>` from the function signature entirely; remove the `#[cfg_attr]` on `world_gpu` (it's used unconditionally). If the architect prefers to keep them for symmetry with potential future use, document why in one line.
- **Restructure tier:** control-flow-tightening / signature-cleanup.

### Item 2 — chunks-RMW pattern + 18% parity gap findings

#### Finding 2A: cross-workgroup write→read on same chunks buffer in the same round is the structural gap (severity: high)

- **Location:**
  - Write site: `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:564` — `chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y);`
  - Own-chunk read (within same workgroup): `:523` — `let cur_chunk_full = chunks_mirror[chunk_idx];`
  - Neighbour-chunk read (CROSS workgroup): `:273` (inside `add_bounds_group`) — `let neighbour_x = chunks_mirror[neighbour_idx].x;`
- **Why this is a problem (structural analysis of the 18% gap):** Within a single regime-2 round, the indirect dispatch launches `count` workgroups (up to `max_group_bound_dispatch = 4096` on wasm). Each workgroup processes one 4³ group → 64 chunks. Workgroup A's chunks are written via `chunks[chunk_idx] = ...`. Workgroup B (a neighbour group, processing a different 4³ subdivision sharing an X/Y/Z face with A) reads A's chunks via `chunks_mirror[neighbour_idx]` — but `chunks_mirror` was last refreshed BEFORE this round began. So B reads A's PRE-this-round value, even though A has now-written its post-expansion value to `chunks`. Result: cross-workgroup AADF propagation within a single round is **lost by one round** — neighbour B always sees A's stale value, never the just-written value. On native with `n_bounds_rounds = 5`, this lag is repaid: round k+1's mirror refresh picks up round k's writes, and over 5 rounds the algorithm converges. On wasm at `n_bounds_rounds = 1`, you get exactly ONE round/frame, so the cross-workgroup propagation lag is locked at one frame. AADF expansion still happens — but every "wave" of expansion crosses 4³-group boundaries one frame later than it should. The ~18% web parity figure (per `13-minimal-fix-verify.md` cleanup-sweep) is consistent with this: ~50 frames × 1 round/frame = 50 cross-boundary hops, vs ~12 frames × 5 rounds = 60 cross-boundary hops on native (which still produces 100% parity because each round's writes feed the next round's reads WITHIN the same encoder via Vulkan's intra-encoder barrier).
- **Suggested direction:** Two structural options for the architect:
  - Option A (small): On wasm only, issue an EXTRA `copy_buffer_to_buffer(chunks → chunks_mirror)` mid-round (after the compute pass, before any subsequent shader that reads chunks). Forces the prior workgroup's writes into the mirror within the same frame. Cost: ~16 MiB copy per frame on wasm. Estimated parity lift: probably small (~5-10%) because it only helps the renderer's first-hit pass, not the next regime-2 round (which is the next frame anyway at n=1).
  - Option B (escape): Restructure the algorithm so cross-workgroup propagation happens via a smaller atomic queue carry — workgroups append the IDs of neighbour groups they "discovered would expand if their AADF was known" to a per-axis FIFO, drained at next round's start. This is the brute-force agent's side-note 1 in `12-brute-force-summary.md:251-261`. Estimated parity lift: 18% → 80%+. Cost: substantial WGSL restructure + algorithmic change.
  - Option C (escape): Raise `n_bounds_rounds` on wasm to 2 with chunks_mirror refresh between them (currently forbidden by `01-context.md:130-131` "DO NOT touch the `n_bounds_rounds = 1` wasm clamp"). Documented for completeness only — the user has pinned this constraint.
- **Classification:** Option A = **SMALL+OBVIOUS+LOW-RISK** (one extra `copy_buffer_to_buffer` call after the round, cfg-gated to wasm). Option B = **ESCAPE** (algorithmic restructure; needs separate session). Option C = **FORBIDDEN** by 01-context constraint.
- **Estimated parity-ratio impact:** Option A: ~5-10% (modest — repays one round of lag, not zero). Option B: ~80%+ (real fix, separate session). The architect should classify Option A as small+obvious+low-risk and surface Option B as a follow-up.

#### Finding 2B: chunks_mirror conflates "cross-frame propagation" with "this-pass shadow" (severity: medium)

- **Location:** `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:117-125` (header) + read sites at `:273` and `:523`.
- **Current state:** The header comment frames `chunks_mirror` as the cross-frame propagation buffer ("refreshed via `copy_buffer_to_buffer(chunks, chunks_mirror, full_size)` between each W3 round so cross-pass reads of AADF state go through a TRANSFER-stage barrier"). But the WGSL uses it as the read-shadow for BOTH the own-chunk read (`:523`, where the writer IS this same workgroup later at `:564`) AND the neighbour-chunk read (`:273`, where the writer is potentially a different workgroup in the same dispatch).
- **Why this is a problem:** The own-chunk read at `:523` is a redundancy from chunks_mirror — on native and within a single workgroup the same thread that reads the own-chunk also writes it; no cross-workgroup hazard. Reading `chunks_mirror` here adds a `copy_buffer_to_buffer`-bandwidth dependency for no correctness reason. The neighbour-chunk read at `:273` IS the load-bearing case — that's where cross-workgroup visibility matters. Conflating both reads into "go through the mirror" obscures which read is the dangerous one.
- **Suggested direction:** Architect can document the split: "Own-chunk reads at `:523` could equally come from `chunks` (writer-equals-reader within workgroup); neighbour reads at `:273` MUST come from `chunks_mirror` (cross-workgroup; writer is potentially another group). The current code unifies both for shader-conditional simplicity." Or — more aggressively — change `:523` to read from `chunks` directly, leaving only `:273` on the mirror. Test would surface whether the unification was load-bearing or cosmetic.
- **Classification:** EXPLORE-ONLY (the unified-mirror is a deliberate post-fix simplification; splitting back may regress the SSIM floor).

#### Finding 2C: chunks-buffer multi-shader mixed-view fragility persists (severity: low)

- **Location:** `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:113-114` (this shader's `array<vec2<u32>>` rw view) + cross-shader: `world_data.wgsl` / `chunk_calc.wgsl` (the other shaders binding the same `chunks_buffer`).
- **Current state:** The `chunks_buffer` is shared across W1 / W3 / first-hit shaders, each with its own view (`array<vec2<u32>>` here non-atomic; W1 originally used `texture_storage_3d<rg32uint, read_write>` before the WebGPU port). Iter-3 (atomicStore on chunks-write at `bounds_calc.wgsl:564`) was an `array<atomic<u32>>` view of the same buffer; it was reverted in `960eeb2` per the inert/non-load-bearing finding. The mixed-view fragility flagged in `12-brute-force-summary.md:271-280` is no longer present in `bounds_calc.wgsl` itself, but the broader multi-shader view-disagreement (texture vs storage, different element types) is still a latent foundation concern.
- **Why this is a problem:** Tint/Dawn's SPIR-V emission for shared-buffer different-element-views is documented as undefined-ish (per `12-brute-force-summary.md:277`); the current tree happens to work, but a future Tint upgrade could regress. Cross-target divergence is also a tell: the WGSL is identical, but native sees 100% parity and wasm sees 18% — that's not the mixed-view per se (which would affect both), it's the cross-workgroup propagation gap (finding 2A). Worth noting for the architect's awareness; not in-scope to fix here.
- **Suggested direction:** Architect should document this as a "latent fragility" in `03-architecture.md`. No in-scope action; this surface is touched only when the cross-workgroup propagation gap is properly addressed (finding 2A, Option B).
- **Classification:** EXPLORE-ONLY.

### Item 3 — `tests.rs` probe-buffer hardcode finding

#### Finding 3A: test probe-history buffer over-allocates 8× and claims "matches production" (severity: low)

- **Location:** `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:525-534`
- **Current state:**
  - Line 525-526 comment: `// 2026-05-19 probe-1B — small probe history buffer for tests (2048 entries × 16 B = 32 KiB; matches production capacity).`
  - Line 529: `size: 2048 * 16,`
  - Line 533: `let probe_zeros: Vec<u32> = vec![0u32; 2048 * 4];`
  - Production const: `crates/bevy_naadf/src/render/construction/mod.rs:340` — `pub const PREPARE_PROBE_HISTORY_ENTRIES: u32 = 256;`. Bytes const at `:343-344` = `(256 × 4 × 4) = 4096` B.
- **Why this is a problem:** Two layers:
  - The numeric drift: test allocates 8× the production size (32 KiB vs 4 KiB). Functionally harmless (WGSL `arrayLength(&prepare_probe_history) / 4u` adapts dynamically; over-allocating tests just means the test's WGSL sees a bigger ring), but couples the test to a stale pre-cleanup-sweep assumption.
  - The misleading comment: claims "matches production capacity" — false post-`c6b0deb`. `14-cleanup-sweep.md:218-222` (cleanup-sweep side-note 2) explicitly flagged this: "I left it as-is for this dispatch (the test over-allocates, which is harmless), but a future /refactor pass should align tests with the production const."
- **Suggested direction:** Replace both `2048 * 16` and `2048 * 4` literals with const expressions: `PREPARE_PROBE_HISTORY_BYTES as usize` (line 529) and `(PREPARE_PROBE_HISTORY_ENTRIES * 4) as usize` (line 533). Imports may need a `use super::super::{PREPARE_PROBE_HISTORY_ENTRIES, PREPARE_PROBE_HISTORY_BYTES};` adjustment (verify path from `bounds_calc/tests.rs`). Update the comment to "matches production capacity (= `PREPARE_PROBE_HISTORY_ENTRIES`)".
- **Restructure tier:** mechanical / constant-alignment.

### Cross-cutting smells (optional)

- **`naadf_bounds_compute_node` is 226 lines, not the brief's "250+".** Verified by `awk` count: function spans `bounds_calc.rs:452-677`. The brief's "250+ lines" is mild over-estimate; the real number is close enough that the architect's docblock-rewrite-and-extract-helper instinct still applies. This is signal for the orchestrator: future briefs should grep/wc-l rather than estimate.
- **The chunks_mirror-related dead-or-misleading docblocks (`bounds_calc.rs:100-104`, `mod.rs:164-167`, `bounds_calc.wgsl:118`) form a cross-file three-way contradiction with the code.** If the architect rewrites one, all three must be rewritten together (or all three deleted). They are repeating the same false claim — pure copy-paste rot.
- **The `[probe1-call]` ring infrastructure (mod.rs:3889-4140) carries its own docblock describing the 256-entry post-fix sizing.** That looks healthy. No finding raised against it. Just noting the contrast: the probe ring's docblock has been kept fresh, while the chunks_mirror docblocks have drifted.

### Side notes / observations / complaints (MANDATORY per CLAUDE.md)

1. **Brief's "250+ lines" estimate was slightly inflated; actual is 226.** Not a blocker — the structural smells are there regardless of exact LOC. But future briefs should `wc -l` the target function or extract the line range from a git blame to avoid this small confidence-eroding drift.

2. **The `end_of_encoder_noop_pipeline` is a textbook example of "dead-but-blast-radius-feels-large".** Removing it is 3-file change (Rust + WGSL + struct field), but it currently has the power to make `naadf_bounds_compute_node` skip whole frames if its pipeline isn't ready (`:527-534`'s early-return). That's a real concrete cost beyond the cosmetic clutter — until that pipeline compiles, regime-2 is dead. If pipeline compilation EVER fails (e.g., a future Tint regression), regime-2 silently stops on wasm. The brief's `[aadf-probe]`/`[probe1-call]` protection list does NOT mention probe-2 / `end_of_encoder_noop` by name — but the architect should confirm with the orchestrator before removing it. I'd argue "non-instrumentation dead-pipeline-with-blast-radius" is a different class than "leave the diagnostic probes alone."

3. **The orchestration anchor docs (12/13/14) are excellent reading — they explicitly tell future agents WHAT was tried, WHAT failed, WHAT was kept, and WHY.** The cleanup-sweep side-notes section already enumerated 4 of the 7 findings I raised here (items 1A 1F 1D 3A). This is what good orchestration scaffolding looks like. Kudos to the prior brute-force + minimal-fix-verify + cleanup-sweep agents; they did the explorer's homework before the explorer was dispatched.

4. **Smell-flag escape clause: the `if let (Some(chunks), Some(chunks_mirror))` pattern at `bounds_calc.rs:635-636` and `:661-662` is duplicated body, easy DRY win.** But I won't flag this as a top-level finding because the architect's natural extract-helper of finding 1F covers the same ground.

5. **One foundation-level concern I want to flag without expanding the brief's scope:** the cross-shader chunks-buffer-view disagreement (W1's storage-texture-vs-storage-buffer port + W3's array<vec2<u32>> view + first-hit shader's read-only view) is exactly the "two addressing schemes for the same buffer" pattern the global CLAUDE.md flags as foundation rot. The brute-force agent's side-note 3 in `12-brute-force-summary.md` already raised it. It's not in-scope for this refactor (small + obvious + low-risk does not cover that surface), but the architect should keep it in mind as a candidate for the FOLLOW-UP "make web parity 100%" session — fix the cross-workgroup propagation gap (finding 2A Option B) AND fix the cross-shader view disagreement at the same time.

6. **Subjective: this codebase carries iter-N intervention narrative very heavily.** I count at least 5 distinct "2026-05-XX brute-force iter-N" / "minimal-fix iter" / "horizon-parity diagnostic" comment markers in the 605-line WGSL alone, and 9+ in the 226-line `naadf_bounds_compute_node`. This is good provenance — every commenter said WHY they did it — but it makes the file read like a changelog rather than a function. The natural endpoint of `/refactor` here is to lift the provenance INTO the orchestration doc trail (where it's already present in 12/13/14) and OUT of the source. Source describes current behavior; orchestration docs describe how-we-got-here.

7. **What I wish I'd had:** A `git log -L :naadf_bounds_compute_node:bounds_calc.rs` view would have been useful to confirm WHICH iter-N comments came from which commit (and whether any are genuinely load-bearing vs all pure history). I worked from the orchestration docs instead — which is appropriate, but somewhat slower. Suggesting orchestrator-prepared `git blame` excerpts for the next round if the architect wants commit-to-comment crosswalks.
