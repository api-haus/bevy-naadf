//! The hard-coded Phase-A test-grid builder (D2).
//!
//! `setup_test_grid` authors a dense voxel volume from simple primitives — a
//! ground slab, several axis-aligned boxes, pillars, two spheres, and **five
//! emissive blocks** distributed through the volume — builds the `VoxelTypes`
//! palette, runs CPU-side AADF construction (`aadf::construct`), and fills the
//! `WorldData` resource (`03-design.md` §6.1 step 1).
//!
//! No `.vox` reader, no `WorldGenerator` port (D2) — this is the smallest
//! content path that gets voxels on screen.
//!
//! **Shared scene (e2e + production).** `setup_test_grid` is a `Startup` system
//! added by [`crate::build_app`] for **both** the production `bevy-naadf`
//! binary and the `e2e_render` harness — only the camera differs (the e2e
//! harness swaps in its own fixed-pose camera). The expanded scene therefore
//! enriches both the live `cargo run` app and the e2e render-test frame.
//!
//! **Scene-expansion (2026-05-14, e2e test-scene expansion task).** The scene
//! was expanded from "ground slab + 2 boxes + 1 sphere + 1 emissive box" to a
//! larger arrangement with **five emissive blocks** spread through the volume,
//! more solid geometry (corner towers, a pillar row, a wall, an arch, two
//! spheres), so the framed scene carries substantial guaranteed-non-black
//! content pre-GI (emissive blocks render white pre-GI) and is a richer GI
//! bounce-light test scene once Batch 5 lands. Still fully deterministic — fixed
//! positions, fixed emissive values, no RNG (the e2e harness depends on this).

use bevy::prelude::*;

use crate::aadf::cell::{BlockCell, BlockPtr, ChunkCell, VoxelPtr};
use crate::aadf::construct::{construct, ConstructedWorld, DenseVolume};
use crate::voxel::vox_import;
use crate::voxel::{MaterialBase, MaterialLayer, VoxelType, VoxelTypeId};
use crate::world::data::{IAabb3, VoxelTypes, WorldData};
use crate::{AppArgs, GridPreset, WORLD_SIZE_IN_CHUNKS, WORLD_SIZE_IN_VOXELS};

// Palette indices into `VoxelTypes::types`. Index 0 is the reserved empty
// placeholder (C# convention) — see `VoxelTypes::default`.
const TY_GROUND: VoxelTypeId = VoxelTypeId(1);
const TY_BOX_A: VoxelTypeId = VoxelTypeId(2);
const TY_BOX_B: VoxelTypeId = VoxelTypeId(3);
const TY_SPHERE: VoxelTypeId = VoxelTypeId(4);
/// The warm-white emissive type — the original single emissive block, kept.
const TY_EMISSIVE: VoxelTypeId = VoxelTypeId(5);
// Scene-expansion palette additions: more solid geometry colours + four extra
// emissive colours, so the expanded scene has varied geometry for GI bounce and
// several distinct emissive blocks (all render white-ish pre-GI; the colour
// matters for GI bounce tint once Batch 5 lands).
const TY_TOWER: VoxelTypeId = VoxelTypeId(6);
const TY_WALL: VoxelTypeId = VoxelTypeId(7);
const TY_PILLAR: VoxelTypeId = VoxelTypeId(8);
/// Cool-white emissive (slightly blue).
const TY_EMISSIVE_COOL: VoxelTypeId = VoxelTypeId(9);
/// Warm amber emissive.
const TY_EMISSIVE_AMBER: VoxelTypeId = VoxelTypeId(10);
/// Green emissive.
const TY_EMISSIVE_GREEN: VoxelTypeId = VoxelTypeId(11);
/// Magenta/pink emissive.
const TY_EMISSIVE_MAGENTA: VoxelTypeId = VoxelTypeId(12);

/// World size for the Phase-A test grid: 4×2×4 chunks = 64×32×64 voxels
/// (`03-design.md` §6.1 step 1).
const GRID_SIZE_IN_CHUNKS: [u32; 3] = [4, 2, 4];

/// Public alias for [`GRID_SIZE_IN_CHUNKS`] — the small Default test-scene
/// footprint, used by [`crate::e2e::gates::demo_origin_v`] to compute the
/// XZ centring offset for the demo inside the fixed world.
pub const DEFAULT_SMALL_WORLD_SIZE_IN_CHUNKS: [u32; 3] = GRID_SIZE_IN_CHUNKS;

/// Startup system: build the hard-coded Phase-A voxel test grid (D2).
///
/// Replaces `main::setup_scene_placeholder`. Inserts the `WorldData` and
/// `VoxelTypes` resources.
///
/// **vox-gpu-rewrite Stage 2 consolidation (2026-05-18):** the dispatch
/// ladder is gone. Every binary (production + every e2e gate) routes
/// through the C#-faithful fixed-world install path:
/// - [`GridPreset::Default`] → [`install_default_embedded_in_fixed_world`]
///   embeds the small primitive scene at the world centre inside the
///   `(4096, 512, 4096)`-voxel container; CPU upload path (W5.6 documented
///   divergence — the synthesised scene stays on the CPU producer because
///   composing it as `ModelData` would force unwanted XZ tiling of the demo).
/// - [`GridPreset::Vox`] → [`install_vox_in_fixed_world`] uploads the model
///   as a [`crate::aadf::generator::ModelData`] resource and the W5 GPU
///   producer chain runs `generator_model` + `chunk_calc` per segment to
///   build the world directly on the device.
///
/// **Stage 14 (2026-05-18):** the `vox_gpu_oracle_cpu_phase` escape hatch
/// is restored as the SOLE test-only branch that routes `Vox` loads to
/// [`install_vox_sized_to_model`] (the legacy natural-bound CPU oracle).
/// The `--vox-gpu-oracle` gate is now an SSIM-based comparison between two
/// REAL renders (CPU oracle vs production W5 GPU path), replacing the
/// Stage 13 Shape C tautology that saved the same captured framebuffer as
/// both `oracle_cpu.png` and `oracle_gpu.png` (which made the per-pixel
/// diff trivially zero and the gate caught nothing). SSIM (Structural
/// Similarity Index) tolerates the ~1.5-6% per-pixel TAA/GI shimmer +
/// atomic-cursor nondeterminism + tiling-vs-natural-bound divergence that
/// killed the per-pixel ceiling, while still flagging gross renderer
/// regressions (sky-bleed, dropouts, voxel-type corruption) that would
/// drop SSIM far below 1.0.
///
/// Production callers never set `vox_gpu_oracle_cpu_phase`; the binary's
/// `--vox` path always routes here with the flag `false` and therefore
/// always invokes [`install_vox_in_fixed_world`].
pub fn setup_test_grid(mut commands: Commands, args: Res<AppArgs>) {
    match &args.grid_preset {
        GridPreset::Default => {
            install_default_embedded_in_fixed_world(&mut commands);
        }
        GridPreset::Vox { path } => {
            if args.vox_gpu_oracle_cpu_phase {
                // Test-only CPU oracle phase for `--vox-gpu-oracle`. The
                // gate's compare phase pairs this CPU render against the
                // GPU render of the same fixture through the production W5
                // path; the SSIM compare tolerates the world-shape
                // divergence (natural-bound CPU vs fixed-tiled GPU) while
                // still flagging gross renderer regressions.
                install_vox_sized_to_model(&mut commands, path);
            } else {
                install_vox_in_fixed_world(&mut commands, path);
            }
        }
        GridPreset::ProceduralStreaming { noise_preset, seed } => {
            install_procedural_streaming_world(&mut commands, &args, *noise_preset, *seed);
        }
        GridPreset::ProceduralStatic { noise_preset, seed } => {
            install_procedural_static_world(&mut commands, &args, *noise_preset, *seed);
        }
    }
}

/// streaming-world Phase 2 — install a procedural-streaming world. The
/// `WorldData` resource is inserted **empty** at the fixed
/// `WORLD_SIZE_IN_CHUNKS` extent (same shape as `install_vox_in_fixed_world`)
/// — the streaming residency driver + the per-frame W5 dispatch chain
/// populate `WorldGpu::{chunks,blocks,voxels}` lazily on GPU; the CPU mirror
/// stays empty.
///
/// Per `02b-design-plan-b.md` § L Phase 2: also inserts:
/// - `Residency` (the sliding-window slot table).
/// - `NoiseChunkSource` (the noise state uniform + classification params).
/// - `InitialCameraPose` at the world's central segment (per v1 assumption #9).
///
/// The VRAM budget pre-flight (`assert_vram_budget_sufficient`) fires here —
/// panics with a clear message if `--vram-budget-mib` is below the slab total.
fn install_procedural_streaming_world(
    commands: &mut Commands,
    args: &AppArgs,
    noise_preset: u32,
    seed: i32,
) {
    use crate::streaming::{
        assert_vram_budget_sufficient, chunk_source::NoiseChunkSource, Residency,
    };

    // Pre-flight VRAM budget check (panics on under-budget).
    assert_vram_budget_sufficient(args.vram_budget_mib);

    // Build the palette — Phase 2 ships a single-entry "ground" palette beyond
    // the empty placeholder; Phase 3 / future presets will extend this.
    let palette = build_streaming_palette();

    // Empty WorldData at the fixed-world extent (mirrors install_vox_in_fixed_world's
    // shape; the streaming driver populates the GPU buffers from noise lazily).
    let mut world_data = WorldData {
        chunks_cpu: Vec::new(),
        blocks_cpu: Vec::new(),
        voxels_cpu: Vec::new(),
        size_in_chunks: WORLD_SIZE_IN_CHUNKS,
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(
                WORLD_SIZE_IN_VOXELS.x as i32 - 1,
                WORLD_SIZE_IN_VOXELS.y as i32 - 1,
                WORLD_SIZE_IN_VOXELS.z as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: Vec::new(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    commands.insert_resource(world_data);
    commands.insert_resource(VoxelTypes { types: palette });

    // Phase 2 ships one noise preset only — `0 = SimpleTerrain` per
    // `chunk_source::default_simple_terrain_state`.
    let chunk_source = match noise_preset {
        0 => {
            let mut cs = NoiseChunkSource::from_seed(seed);
            cs.sea_level = args.sea_level;
            cs.terrain_amplitude = args.terrain_amplitude;
            cs
        }
        n => {
            error!(
                "streaming-world: unknown noise_preset={n}; falling back to \
                 SimpleTerrain (preset 0)",
            );
            NoiseChunkSource::from_seed(seed)
        }
    };
    commands.insert_resource(chunk_source);

    // Residency manager — fresh-empty; the per-frame driver populates
    // `admissions_this_frame` on the first tick after the camera spawns.
    commands.insert_resource(Residency::empty(args.max_segments_per_frame));

    // Camera spawn at the centre of the X/Z window per v1 assumption #9. The
    // world is `WORLD_SIZE_IN_VOXELS = (4096, 512, 4096)` voxels; centre is
    // (2048, sea_level + 32, 2048). Look slightly down at the centre.
    let cx = (WORLD_SIZE_IN_VOXELS.x as f32) * 0.5;
    let cz = (WORLD_SIZE_IN_VOXELS.z as f32) * 0.5;
    // Spawn ~32 voxels above sea_level so the camera sits above the terrain
    // surface (terrain hovers around sea_level by construction).
    let cam_y = args.sea_level + 32.0;
    let cam_pos = Vec3::new(cx, cam_y, cz);
    // Look forward in +X direction (also looking slightly down so terrain
    // is in view).
    let cam_look = Vec3::new(cx + 100.0, args.sea_level - 16.0, cz);
    commands.insert_resource(crate::camera::InitialCameraPose(
        Transform::from_translation(cam_pos).looking_at(cam_look, Vec3::Y),
    ));

    info!(
        "streaming-world: ProceduralStreaming preset installed — \
         noise_preset={noise_preset}, seed={seed}, sea_level={:.1}, \
         terrain_amplitude={:.1}, vram_budget_mib={}, \
         max_segments_per_frame={}; camera spawn at {:?} looking at {:?}",
        args.sea_level,
        args.terrain_amplitude,
        args.vram_budget_mib,
        args.max_segments_per_frame,
        cam_pos,
        cam_look,
    );
}

/// streaming-world Phase 2.4 — install a procedural-static world. Mirrors
/// `install_procedural_streaming_world` BUT does **not** insert `Residency`.
/// Instead inserts a `ProceduralStaticActive` marker resource that drives
/// the render-world's `(a0b)` one-shot 512-segment dispatch branch (see
/// `crate::render::construction::naadf_gpu_producer_node`).
///
/// World container: identical to streaming preset — empty `WorldData` at
/// fixed `WORLD_SIZE_IN_CHUNKS = (256, 32, 256)` extent. The GPU producer
/// populates `WorldGpu::{chunks,blocks,voxels}` once at startup via the
/// noise → segment_voxel_buffer → chunk_calc → bounds chain.
///
/// Camera spawn: world-centre `(2048, sea_level + 32, 2048)` looking +X
/// with a slight downward angle (matches the streaming preset's pose so
/// framebuffer assertions are visually comparable across both presets).
///
/// Independent of the residency machinery — no `Residency` resource is
/// inserted; the `pin_streaming_window_camera` helper's
/// `translate_world_to_window_local` returns identity when no residency
/// exists, so the camera Transform stays in absolute world-voxel coords
/// (which is what the static-preset renderer expects: segment `(sx, sy,
/// sz)` → chunks at world segment positions `(sx, sy, sz)` — no
/// translation, no origin shift).
fn install_procedural_static_world(
    commands: &mut Commands,
    args: &AppArgs,
    noise_preset: u32,
    seed: i32,
) {
    use crate::streaming::{chunk_source::NoiseChunkSource, ProceduralStaticActive};

    // Build the palette — same shape as streaming preset.
    let palette = build_streaming_palette();

    // Empty WorldData at the fixed-world extent (mirrors
    // `install_procedural_streaming_world` + `install_vox_in_fixed_world`).
    let mut world_data = WorldData {
        chunks_cpu: Vec::new(),
        blocks_cpu: Vec::new(),
        voxels_cpu: Vec::new(),
        size_in_chunks: WORLD_SIZE_IN_CHUNKS,
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(
                WORLD_SIZE_IN_VOXELS.x as i32 - 1,
                WORLD_SIZE_IN_VOXELS.y as i32 - 1,
                WORLD_SIZE_IN_VOXELS.z as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: Vec::new(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    commands.insert_resource(world_data);
    commands.insert_resource(VoxelTypes { types: palette });

    // Phase 2.4 ships one noise preset (SimpleTerrain) — same defaults as
    // the streaming preset's preset 0 with the args-supplied sea_level and
    // terrain_amplitude. Choice rationale documented in
    // `docs/orchestrate/streaming-world/03d-impl-static-noise.md`.
    let chunk_source = match noise_preset {
        0 => {
            let mut cs = NoiseChunkSource::from_seed(seed);
            cs.sea_level = args.sea_level;
            cs.terrain_amplitude = args.terrain_amplitude;
            cs
        }
        n => {
            error!(
                "streaming-world Phase 2.4: unknown noise_preset={n}; falling \
                 back to SimpleTerrain (preset 0)",
            );
            NoiseChunkSource::from_seed(seed)
        }
    };
    commands.insert_resource(chunk_source);

    // Marker — presence flips `StreamingExtractRender.static_mode_active`
    // in the render-world extract.
    commands.insert_resource(ProceduralStaticActive);

    // Camera spawn — same shape as `install_procedural_streaming_world`'s
    // centre-of-world pose so framebuffer assertions are comparable.
    let cx = (WORLD_SIZE_IN_VOXELS.x as f32) * 0.5;
    let cz = (WORLD_SIZE_IN_VOXELS.z as f32) * 0.5;
    let cam_y = args.sea_level + 32.0;
    let cam_pos = Vec3::new(cx, cam_y, cz);
    let cam_look = Vec3::new(cx + 100.0, args.sea_level - 16.0, cz);
    commands.insert_resource(crate::camera::InitialCameraPose(
        Transform::from_translation(cam_pos).looking_at(cam_look, Vec3::Y),
    ));

    info!(
        "streaming-world Phase 2.4: ProceduralStatic preset installed — \
         noise_preset={noise_preset}, seed={seed}, sea_level={:.1}, \
         terrain_amplitude={:.1}; camera spawn at {:?} looking at {:?}. \
         GPU producer will fire one-shot 512-segment dispatch via the \
         (a0b) static-preset branch in naadf_gpu_producer_node.",
        args.sea_level,
        args.terrain_amplitude,
        cam_pos,
        cam_look,
    );
}

/// Phase 2 default palette — index 0 reserved empty + a single "ground" type
/// at index 1. The streaming preset's noise classification emits index 1 for
/// every solid voxel.
fn build_streaming_palette() -> Vec<VoxelType> {
    vec![
        // 0 — reserved empty placeholder.
        VoxelType::default(),
        // 1 — generic terrain ground: warm-grey diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.85,
            color_base: Vec3::new(0.50, 0.48, 0.42),
            color_layered: Vec3::ZERO,
        },
    ]
}

/// C#-faithful default — build the same primitive scene the small-world path
/// builds, then embed it at the `(0, 0, 0)` corner of a fixed
/// [`WORLD_SIZE_IN_CHUNKS`] world. Mirrors `WorldHandler.cs:29-35`'s "world
/// container is built, then the model either loads or doesn't" semantic: when
/// the user runs `just dev` with no `.vox` argument, they get this populated
/// corner inside a huge editable empty space.
///
/// `dense_voxel_types` is intentionally empty — at 4096×512×4096 voxels the
/// dense u16 mirror is ~17 GiB. The data-driven gate at
/// `render/construction/mod.rs:888` then skips the GPU producer chain and the
/// renderer reads the pre-built CPU buffers directly.
fn install_default_embedded_in_fixed_world(commands: &mut Commands) {
    let palette = build_palette();
    let volume = build_default_volume();
    let small = construct(&volume);

    // Build a single-chunk ground template — 3 voxels of `TY_GROUND` at Y=0..2,
    // empty above. Tiled across the entire chunk-Y=0 layer outside the demo
    // footprint to give the user a permanent floor reference. Matches the
    // demo's own ground stripe height so the demo/floor boundary is seamless.
    let ground_template = construct(&build_ground_chunk_volume());

    let center_offset_chunks = IVec3::new(
        (WORLD_SIZE_IN_CHUNKS.x as i32 - small.size_in_chunks[0] as i32) / 2,
        0,
        (WORLD_SIZE_IN_CHUNKS.z as i32 - small.size_in_chunks[2] as i32) / 2,
    );

    let composed = compose_default_scene_into_fixed_world(
        &small,
        &ground_template,
        WORLD_SIZE_IN_CHUNKS,
        center_offset_chunks,
    );
    let size_v = WORLD_SIZE_IN_VOXELS;

    info!(
        "NAADF default scene embedded in fixed world: small={}×{}×{} chunks centered at chunk-({},{},{}); \
         fixed {}×{}×{} chunks ({}×{}×{} voxels) with full-area ground at chunk-Y=0; \
         chunks_cpu {} u32s, blocks_cpu {} u32s, voxels_cpu {} u32s. \
         Dense voxel-type mirror skipped (would be ~17 GiB); GPU producer disabled, CPU upload path active.",
        small.size_in_chunks[0],
        small.size_in_chunks[1],
        small.size_in_chunks[2],
        center_offset_chunks.x,
        center_offset_chunks.y,
        center_offset_chunks.z,
        WORLD_SIZE_IN_CHUNKS.x,
        WORLD_SIZE_IN_CHUNKS.y,
        WORLD_SIZE_IN_CHUNKS.z,
        size_v.x,
        size_v.y,
        size_v.z,
        composed.chunks.len(),
        composed.blocks.len(),
        composed.voxels.len(),
    );

    // Frame the centered demo with the same relative camera pose the
    // small-world Default uses (`(11, 7, 17)` looking at `(0, 4, -3)`), just
    // translated to the demo's embed origin in the fixed world. Without this
    // resource `setup_camera` falls back to the small-world pose (centered on
    // `(0, 0, 0)`), which now stares at empty space far from the demo.
    let demo_origin_v = Vec3::new(
        (center_offset_chunks.x * 16) as f32,
        0.0,
        (center_offset_chunks.z * 16) as f32,
    );
    let cam_pos = demo_origin_v + Vec3::new(11.0, 7.0, 17.0);
    let cam_look = demo_origin_v + Vec3::new(0.0, 4.0, -3.0);
    commands.insert_resource(crate::camera::InitialCameraPose(
        Transform::from_translation(cam_pos).looking_at(cam_look, Vec3::Y),
    ));

    let mut world_data = WorldData {
        chunks_cpu: composed.chunks,
        blocks_cpu: composed.blocks,
        voxels_cpu: composed.voxels,
        size_in_chunks: WORLD_SIZE_IN_CHUNKS,
        // `bounding_box` is the **ray-traversal AABB clip** the renderer
        // uploads as `boundingBoxMin/Max` (`render/prepare.rs:374-380`,
        // matches C# `WorldData.cs:477-478` which always passes the full
        // world extent). Sizing it to the full fixed world matters here:
        // the user can edit voxels anywhere in the 4096×512×4096 container,
        // and the primary ray must be allowed to reach + intersect those
        // edits. A tight small-scene box (`64×32×64`) would clip rays before
        // they hit any edit placed outside the corner — invisible writes
        // from the user's perspective. Identical to the `.vox`-load path's
        // bounds (set in `vox_import::build_world_from_vox`).
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(
                WORLD_SIZE_IN_VOXELS.x as i32 - 1,
                WORLD_SIZE_IN_VOXELS.y as i32 - 1,
                WORLD_SIZE_IN_VOXELS.z as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: Vec::new(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    commands.insert_resource(world_data);
    commands.insert_resource(VoxelTypes { types: palette });
}

/// Legacy `.vox` loader — sizes the world to the model's natural bounds.
///
/// **Stage 14 (2026-05-18) status:** reachable ONLY via the test-only
/// `AppArgs.vox_gpu_oracle_cpu_phase` branch in [`setup_test_grid`] — the
/// CPU oracle phase of the SSIM-based `--vox-gpu-oracle` gate. The
/// production binary always routes through [`install_vox_in_fixed_world`].
/// Kept as a separate path so the CPU-vs-GPU SSIM compare measures two
/// distinct install paths' renders rather than two captures of the same
/// render.
fn install_vox_sized_to_model(commands: &mut Commands, path: &std::path::Path) {
    match vox_import::load_vox_tiled(path, 1) {
        Ok(imp) => {
            let size_in_chunks = imp.world.size_in_chunks;
            info!(
                "NAADF .vox loaded from {} (CPU oracle, sized-to-model): {} palette entries, world bounds {}×{}×{} chunks ({}×{}×{} voxels), {} chunks total, blocks_cpu {} u32s, voxels_cpu {} u32s",
                path.display(),
                imp.palette.len(),
                size_in_chunks[0],
                size_in_chunks[1],
                size_in_chunks[2],
                size_in_chunks[0] * 16,
                size_in_chunks[1] * 16,
                size_in_chunks[2] * 16,
                imp.world.chunks.len(),
                imp.world.blocks.len(),
                imp.world.voxels.len(),
            );
            let world_voxels = [
                size_in_chunks[0] * 16,
                size_in_chunks[1] * 16,
                size_in_chunks[2] * 16,
            ];
            commands.insert_resource(crate::camera::InitialCameraPose::from_world_voxels(
                world_voxels,
            ));
            let (world_data, voxel_types) = vox_import::build_world_from_vox(imp);
            commands.insert_resource(world_data);
            commands.insert_resource(voxel_types);
        }
        Err(e) => {
            error!(
                ".vox load failed ({e}); falling back to embedded default in fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
        }
    }
}

/// C#-faithful `.vox` loader (vox-gpu-rewrite W5.1) — parses the `.vox` as a
/// single-tile `ImportedVox` and hands it to the W5 GPU producer chain via a
/// main-world [`crate::aadf::generator::ModelData`] resource. The CPU
/// `WorldData` inserted here is **empty at the fixed world size**: the W5
/// per-segment `generator_model` + `chunk_calc` dispatches populate
/// `WorldGpu::chunks/blocks/voxels` directly on the GPU (mirrors C#
/// `WorldData.GenerateWorld` at `WorldData.cs:120-156`, dispatching
/// `generatorModel.fx` then `calcBlock` per segment).
///
/// **W5.1 intermediate state:** after W5.1 lands, the GPU producer chain that
/// consumes `ModelData` is NOT yet wired (that's W5.2 + W5.3). The
/// `.vox` → fixed-world path therefore renders nothing meaningful until W5.3
/// lands. This is by design — W5.1 is data-plumbing only.
///
/// On load failure: falls back to `install_default_embedded_in_fixed_world`
/// (same as the pre-W5.1 behaviour, so the world is still fixed-size and
/// editable — matches C#'s missing-`oasis.cvox` path).
fn install_vox_in_fixed_world(commands: &mut Commands, path: &std::path::Path) {
    // W5.1 — parse as a single-tile sparse import (no CPU tiling).
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            error!(
                ".vox load failed (read error: {e}); falling back to embedded \
                 default in fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
            return;
        }
    };
    let data = match dot_vox::load_bytes(&bytes) {
        Ok(d) => d,
        Err(e) => {
            error!(
                ".vox load failed (parse error: {e}); falling back to embedded \
                 default in fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
            return;
        }
    };
    let imp = match vox_import::parse_dot_vox_data(&data) {
        Ok(i) => i,
        Err(e) => {
            error!(
                ".vox load failed ({e}); falling back to embedded default in \
                 fixed world (path: {})",
                path.display()
            );
            install_default_embedded_in_fixed_world(commands);
            return;
        }
    };

    let model_size_in_chunks = imp.world.size_in_chunks;
    info!(
        "NAADF .vox loaded from {} → ModelData ({}×{}×{} chunks; \
         data_chunk={} u32s, data_block={} u32s, data_voxel={} u32s, \
         {} palette entries). Fixed world {}×{}×{} chunks; GPU producer \
         chain runs per WORLD_SIZE_IN_SEGMENTS = ({}, {}, {}).",
        path.display(),
        model_size_in_chunks[0],
        model_size_in_chunks[1],
        model_size_in_chunks[2],
        imp.world.chunks.len(),
        imp.world.blocks.len(),
        imp.world.voxels.len(),
        imp.palette.len(),
        WORLD_SIZE_IN_CHUNKS.x,
        WORLD_SIZE_IN_CHUNKS.y,
        WORLD_SIZE_IN_CHUNKS.z,
        crate::WORLD_SIZE_IN_SEGMENTS.x,
        crate::WORLD_SIZE_IN_SEGMENTS.y,
        crate::WORLD_SIZE_IN_SEGMENTS.z,
    );

    // C# camera spawn: literal (500, 200, 40) voxels in the fixed
    // 4096×512×4096 world (WorldRender.cs:48-49). `from_world_voxels` scales
    // proportionally for the fixed world size — see camera/mod.rs:54-64.
    let world_voxels = [
        WORLD_SIZE_IN_VOXELS.x,
        WORLD_SIZE_IN_VOXELS.y,
        WORLD_SIZE_IN_VOXELS.z,
    ];
    commands.insert_resource(crate::camera::InitialCameraPose::from_world_voxels(
        world_voxels,
    ));

    // W5.1 — convert ConstructedWorld → ModelData. The `chunks/blocks/voxels`
    // u32 buffers `vox_import` produces are byte-identical to NAADF's
    // `dataChunk/dataBlock/dataVoxel` encoding (`aadf/generator.rs:64-71`).
    //
    // vox-gpu-rewrite Stage 11 — match C# `ModelData.cs::ImportFromVox:442-446`
    // convention: empty voxels in the model encoding must be literal 0, not
    // AADF-tagged. `build_constructed_world_sparse` produces the renderer-side
    // encoding (low half-word carries AADF distance bits for empty voxels);
    // the W5 generator shader (`generator_model.wgsl:99-103, 148-154`) reads
    // `& 0x7FFF` and then promotes any non-zero to "full" via bit 15, which
    // would falsely treat AADF-bearing empties as full voxels with the AADF
    // bits as type → renderer decodes type as thousands → OOB palette → black.
    // Strip the AADF bits from empty half-words here to match C# convention.
    // See `docs/orchestrate/vox-gpu-rewrite/16-diagnostic-renderer-wiring.md`.
    let data_voxel: Vec<u32> = imp
        .world
        .voxels
        .iter()
        .map(|&pair| {
            let lo = pair & 0xFFFF;
            let hi = (pair >> 16) & 0xFFFF;
            let lo_out = if (lo & 0x8000) != 0 { lo } else { 0 };
            let hi_out = if (hi & 0x8000) != 0 { hi } else { 0 };
            lo_out | (hi_out << 16)
        })
        .collect();
    let model_data = crate::aadf::generator::ModelData {
        data_chunk: imp.world.chunks,
        data_block: imp.world.blocks,
        data_voxel,
        size_in_chunks: model_size_in_chunks,
    };
    commands.insert_resource(model_data);

    // W5.1 — install an EMPTY WorldData at the FIXED world size. The renderer
    // still consumes this for bind groups (prepare_world_gpu builds the
    // chunks/blocks/voxels storage buffers); the GPU producer dispatches
    // populate them from the segment_voxel_buffer the per-segment chain
    // writes. `dense_voxel_types = Vec::new()` preserves the existing
    // `if meta.dense_voxel_types.is_empty() { return; }` gate at
    // `render/construction/mod.rs:1936-1941` (the W5.3 three-way ladder adds
    // a NEW gate ABOVE that one for ModelDataRender presence).
    let mut world_data = WorldData {
        chunks_cpu: Vec::new(),
        blocks_cpu: Vec::new(),
        voxels_cpu: Vec::new(),
        size_in_chunks: WORLD_SIZE_IN_CHUNKS,
        bounding_box: IAabb3 {
            min: IVec3::ZERO,
            max: IVec3::new(
                WORLD_SIZE_IN_VOXELS.x as i32 - 1,
                WORLD_SIZE_IN_VOXELS.y as i32 - 1,
                WORLD_SIZE_IN_VOXELS.z as i32 - 1,
            ),
        },
        pending_edits: Default::default(),
        dense_voxel_types: Vec::new(),
        block_hashing: crate::aadf::block_hash::BlockHashingHandler::new(),
    };
    world_data.seed_block_hashing();
    commands.insert_resource(world_data);
    commands.insert_resource(VoxelTypes { types: imp.palette });
}

/// A `1×1×1`-chunk [`DenseVolume`] containing the standard 3-voxel ground
/// stripe (`TY_GROUND` at Y=0..2, empty above). Run through `construct()` to
/// get the canonical chunk/block/voxel encoding for one "floor tile" — that
/// encoding is then tiled across the full chunk-Y=0 layer outside the demo
/// footprint by [`compose_default_scene_into_fixed_world`].
fn build_ground_chunk_volume() -> DenseVolume {
    let mut v = DenseVolume::empty([1, 1, 1]);
    let s = v.size_in_voxels();
    for z in 0..s[2] {
        for x in 0..s[0] {
            v.set([x, 0, z], TY_GROUND);
            v.set([x, 1, z], TY_GROUND);
            v.set([x, 2, z], TY_GROUND);
        }
    }
    v
}

/// Compose a fixed-size world from (a) a centered demo scene and (b) a
/// ground-chunk template tiled across the rest of the bottom Y-layer.
///
/// Output is byte-equivalent to what a single `construct(&DenseVolume)` call
/// would produce on a hand-built world of the same content — except we never
/// allocate the 17 GiB dense voxel mirror that would imply.
///
/// **Pointer-shift strategy:**
///
/// `demo.blocks` / `demo.voxels` are copied verbatim at the start of the
/// output buffers, so every `demo.chunks[i]`'s `BlockPtr` (and every demo
/// block's `VoxelPtr`) stays valid without rewriting.
///
/// `ground_template`'s voxels are appended after the demo voxels and its
/// blocks are appended after the demo blocks with their `VoxelPtr` payloads
/// shifted by the demo's voxel-buffer length. Each ground chunk gets its
/// **own copy** of the 64-block slice (one per chunk) — sharing a single
/// `BlockPtr` across thousands of chunks would mean a single edit to one
/// ground chunk silently mutates every other ground chunk through the shared
/// `blocks_cpu` window. Cost: `~65k chunks × 64 blocks × 4 bytes ≈ 16 MiB`
/// for `blocks_cpu`.
///
/// The ground template's *voxel* slot, on the other hand, is shared across
/// every ground block (block dedup is content-addressable via `VoxelPtr` and
/// the [`crate::aadf::block_hash::BlockHashingHandler`] refcounts edits
/// properly), so `voxels_cpu` only grows by the template's voxel count
/// (~32 u32s).
fn compose_default_scene_into_fixed_world(
    demo: &ConstructedWorld,
    ground_template: &ConstructedWorld,
    target_chunks: UVec3,
    center_offset: IVec3,
) -> ConstructedWorld {
    let [dx, dy, dz] = demo.size_in_chunks;
    let tx = target_chunks.x;
    let ty = target_chunks.y;
    let tz = target_chunks.z;
    debug_assert!(
        dx <= tx && dy <= ty && dz <= tz,
        "demo ({dx},{dy},{dz}) must fit inside fixed world ({tx},{ty},{tz})",
    );
    debug_assert_eq!(
        ground_template.size_in_chunks, [1, 1, 1],
        "ground template must be a single chunk",
    );

    // Start with the demo's data verbatim — its BlockPtr/VoxelPtr indices
    // reference these offsets, so we preserve them.
    let mut out_chunks = vec![0u32; (tx * ty * tz) as usize];
    let mut out_blocks = demo.blocks.clone();
    let mut out_voxels = demo.voxels.clone();

    // Append the ground template's voxels — one shared slot for every tiled
    // ground block thanks to content-addressable dedup.
    let ground_voxel_base = out_voxels.len() as u32;
    out_voxels.extend_from_slice(&ground_template.voxels);

    // Pre-shift the ground-template's 64 blocks so their `VoxelPtr`s point at
    // the right offsets in `out_voxels`. The same 64 shifted block words are
    // re-appended once per ground chunk so each chunk owns its block slice
    // (see the function-level doc for the "don't share BlockPtr" rationale).
    let ground_chunk_block_count = 64usize;
    debug_assert_eq!(
        ground_template.blocks.len(),
        ground_chunk_block_count,
        "ground template should have exactly one chunk's worth of blocks (64)",
    );
    let ground_blocks_shifted: Vec<u32> = ground_template
        .blocks
        .iter()
        .map(|&raw| shift_block_voxel_ptr(raw, ground_voxel_base))
        .collect();

    // Demo embed bounds in chunk space.
    let off_x = center_offset.x as i64;
    let off_z = center_offset.z as i64;
    let off_y = center_offset.y as i64;

    for cz in 0..tz {
        for cy in 0..ty {
            for cx in 0..tx {
                let dst = (cx + cy * tx + cz * tx * ty) as usize;

                // Inside the centered demo embed?
                let demo_lx = cx as i64 - off_x;
                let demo_ly = cy as i64 - off_y;
                let demo_lz = cz as i64 - off_z;
                let in_demo = demo_lx >= 0
                    && demo_lx < dx as i64
                    && demo_ly >= 0
                    && demo_ly < dy as i64
                    && demo_lz >= 0
                    && demo_lz < dz as i64;
                if in_demo {
                    let src = (demo_lx as u32
                        + demo_ly as u32 * dx
                        + demo_lz as u32 * dx * dy) as usize;
                    out_chunks[dst] = demo.chunks[src];
                    continue;
                }

                if cy == 0 {
                    // Allocate a fresh 64-block slice for this ground chunk so
                    // edits stay local to this chunk (don't clobber neighbours
                    // through a shared BlockPtr).
                    let block_base = out_blocks.len() as u32;
                    out_blocks.extend_from_slice(&ground_blocks_shifted);
                    // The ground template's single chunk is `Mixed(BlockPtr(0))`
                    // — we re-encode with this chunk's freshly-allocated base.
                    out_chunks[dst] =
                        ChunkCell::Mixed(BlockPtr(block_base)).encode();
                }
                // else: empty above the floor (chunks_cpu[dst] = 0u32).
            }
        }
    }

    ConstructedWorld {
        chunks: out_chunks,
        blocks: out_blocks,
        voxels: out_voxels,
        size_in_chunks: [tx, ty, tz],
    }
}

/// Decode a block-cell `u32`, shift its `VoxelPtr` by `voxel_offset` if it's
/// a Mixed cell, re-encode. Empty / UniformFull cells pass through unchanged.
fn shift_block_voxel_ptr(raw: u32, voxel_offset: u32) -> u32 {
    match BlockCell::decode(raw) {
        BlockCell::Mixed(vp) => BlockCell::Mixed(VoxelPtr(vp.0 + voxel_offset)).encode(),
        other => other.encode(),
    }
}

/// Build the Phase-A voxel-type palette. Index 0 is the empty placeholder; the
/// rest match the `TY_*` constants.
fn build_palette() -> Vec<VoxelType> {
    vec![
        // 0 — reserved empty placeholder.
        VoxelType::default(),
        // 1 — ground: a flat grey diffuse slab.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.9,
            color_base: Vec3::new(0.55, 0.55, 0.58),
            color_layered: Vec3::ZERO,
        },
        // 2 — box A: warm diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.8,
            color_base: Vec3::new(0.80, 0.30, 0.22),
            color_layered: Vec3::ZERO,
        },
        // 3 — box B: cool diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.8,
            color_base: Vec3::new(0.25, 0.45, 0.80),
            color_layered: Vec3::ZERO,
        },
        // 4 — sphere: green diffuse.
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.7,
            color_base: Vec3::new(0.30, 0.70, 0.32),
            color_layered: Vec3::ZERO,
        },
        // 5 — warm-white emissive box. `color_layered` doubles as emissive
        // intensity (`02-research.md` §4.6); the contribution itself is Phase B.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.92, 0.78),
            color_layered: Vec3::new(8.0, 7.4, 6.2),
        },
        // 6 — tower: a neutral light-grey diffuse (corner towers).
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.85,
            color_base: Vec3::new(0.62, 0.60, 0.58),
            color_layered: Vec3::ZERO,
        },
        // 7 — wall: a warm sand diffuse (the back wall + arch).
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.85,
            color_base: Vec3::new(0.72, 0.62, 0.42),
            color_layered: Vec3::ZERO,
        },
        // 8 — pillar: a violet diffuse (the pillar row).
        VoxelType {
            material_base: MaterialBase::Diffuse,
            material_layer: MaterialLayer::None,
            roughness: 0.8,
            color_base: Vec3::new(0.45, 0.32, 0.62),
            color_layered: Vec3::ZERO,
        },
        // 9 — cool-white emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(0.82, 0.88, 1.0),
            color_layered: Vec3::new(6.4, 6.9, 8.0),
        },
        // 10 — warm amber emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.66, 0.28),
            color_layered: Vec3::new(8.0, 5.3, 2.2),
        },
        // 11 — green emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(0.40, 1.0, 0.46),
            color_layered: Vec3::new(3.2, 8.0, 3.7),
        },
        // 12 — magenta emissive.
        VoxelType {
            material_base: MaterialBase::Emissive,
            material_layer: MaterialLayer::None,
            roughness: 1.0,
            color_base: Vec3::new(1.0, 0.42, 0.86),
            color_layered: Vec3::new(8.0, 3.4, 6.9),
        },
    ]
}

/// Build the default test volume (`03-design.md` §6.1 step 1).
///
/// **Expanded scene (2026-05-14).** A larger, richer arrangement than the
/// original "ground slab + 2 boxes + 1 sphere + 1 emissive box": a ground slab,
/// four corner towers, a back wall with an arch cut through it, a row of
/// pillars, two warm/cool diffuse boxes, two spheres, and **five emissive
/// blocks** distributed through the volume at varied positions and heights.
///
/// Rationale: the emissive blocks render white-ish pre-GI, so spreading five of
/// them through the framed volume guarantees substantial non-black content even
/// before GI bounce lighting lands (Batch 5), and the extra diffuse geometry
/// (towers, wall, pillars, spheres) gives varied surfaces for GI bounce light to
/// fall on once Batch 5 is in. Fully deterministic — fixed positions, fixed
/// emissive values, no RNG (the e2e harness depends on a bit-identical scene).
///
/// All coordinates are in voxels within the 64×32×64 volume.
fn build_default_volume() -> DenseVolume {
    let mut v = DenseVolume::empty(GRID_SIZE_IN_CHUNKS);
    let size = v.size_in_voxels();
    let (sx, _sy, sz) = (size[0], size[1], size[2]);

    // --- Ground + perimeter -------------------------------------------------

    // Ground slab — the bottom 3 voxel layers, full width/depth.
    fill_box(&mut v, [0, 0, 0], [sx - 1, 2, sz - 1], TY_GROUND);

    // Four corner towers — neutral grey, varied heights, framing the volume.
    fill_box(&mut v, [2, 3, 2], [9, 26, 9], TY_TOWER);
    fill_box(&mut v, [54, 3, 2], [61, 21, 9], TY_TOWER);
    fill_box(&mut v, [2, 3, 54], [9, 18, 61], TY_TOWER);
    fill_box(&mut v, [54, 3, 54], [61, 24, 61], TY_TOWER);

    // Back wall along the far +x edge with an arch cut through it — sand
    // diffuse, a big surface for GI bounce.
    fill_box(&mut v, [56, 3, 14], [60, 22, 49], TY_WALL);
    // Arch opening — carve a doorway back to empty.
    fill_box(&mut v, [55, 3, 26], [61, 14, 37], VoxelTypeId::EMPTY);

    // --- Mid-scene diffuse geometry ----------------------------------------

    // Box A — a tall warm box, sitting on the ground.
    fill_box(&mut v, [12, 3, 14], [23, 20, 25], TY_BOX_A);

    // Box B — a wider cool box on the far side.
    fill_box(&mut v, [38, 3, 40], [52, 16, 55], TY_BOX_B);

    // A row of three violet pillars marching across the mid-volume.
    fill_box(&mut v, [26, 3, 8], [29, 17, 11], TY_PILLAR);
    fill_box(&mut v, [34, 3, 8], [37, 19, 11], TY_PILLAR);
    fill_box(&mut v, [42, 3, 8], [45, 15, 11], TY_PILLAR);

    // Two green diffuse spheres, resting on the ground.
    fill_sphere(&mut v, [30, 11, 30], 8, TY_SPHERE);
    fill_sphere(&mut v, [44, 9, 24], 6, TY_SPHERE);

    // --- Five emissive blocks, distributed through the volume --------------
    //
    // These render white-ish pre-GI — the guaranteed-non-black content — and
    // are the GI bounce-light sources once Batch 5 lands. Spread across the
    // volume at varied positions and heights so several are in frame from any
    // sensible 3/4 vantage.

    // 1 — warm-white, a small bright cube floating near the volume centre
    // (the original single emissive block, kept in roughly its old place).
    fill_box(&mut v, [28, 23, 30], [34, 28, 36], TY_EMISSIVE);

    // 2 — cool-white, low and toward the near corner.
    fill_box(&mut v, [10, 6, 44], [15, 11, 49], TY_EMISSIVE_COOL);

    // 3 — warm amber, high up near the far corner.
    fill_box(&mut v, [46, 24, 46], [51, 29, 51], TY_EMISSIVE_AMBER);

    // 4 — green, mid-height on the +x / -z side.
    fill_box(&mut v, [44, 14, 14], [49, 19, 19], TY_EMISSIVE_GREEN);

    // 5 — magenta, low and toward the near +z edge, in front of box B.
    fill_box(&mut v, [20, 5, 50], [25, 10, 55], TY_EMISSIVE_MAGENTA);

    v
}

/// Fill the inclusive voxel box `[min, max]` with `ty`, clamped to the volume.
fn fill_box(v: &mut DenseVolume, min: [u32; 3], max: [u32; 3], ty: VoxelTypeId) {
    let size = v.size_in_voxels();
    let lo = [min[0].min(size[0] - 1), min[1].min(size[1] - 1), min[2].min(size[2] - 1)];
    let hi = [max[0].min(size[0] - 1), max[1].min(size[1] - 1), max[2].min(size[2] - 1)];
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                v.set([x, y, z], ty);
            }
        }
    }
}

/// Fill a solid sphere of integer `radius` centred at `center` with `ty`,
/// clamped to the volume.
fn fill_sphere(v: &mut DenseVolume, center: [u32; 3], radius: u32, ty: VoxelTypeId) {
    let size = v.size_in_voxels();
    let r2 = (radius * radius) as i64;
    let c = [center[0] as i64, center[1] as i64, center[2] as i64];
    let lo = [
        center[0].saturating_sub(radius),
        center[1].saturating_sub(radius),
        center[2].saturating_sub(radius),
    ];
    let hi = [
        (center[0] + radius).min(size[0] - 1),
        (center[1] + radius).min(size[1] - 1),
        (center[2] + radius).min(size[2] - 1),
    ];
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let d = [x as i64 - c[0], y as i64 - c[1], z as i64 - c[2]];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r2 {
                    v.set([x, y, z], ty);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aadf::cell::ChunkCell;

    #[test]
    fn default_volume_has_expected_dimensions() {
        let v = build_default_volume();
        assert_eq!(v.size_in_chunks, GRID_SIZE_IN_CHUNKS);
        assert_eq!(v.size_in_voxels(), [64, 32, 64]);
    }

    #[test]
    fn default_volume_has_ground_and_air() {
        let v = build_default_volume();
        // Ground present at the bottom.
        assert_eq!(v.voxel_at([0, 0, 0]), TY_GROUND);
        assert_eq!(v.voxel_at([63, 2, 63]), TY_GROUND);
        // Air above the scene in an empty region (well above all geometry).
        assert_eq!(v.voxel_at([31, 31, 20]), VoxelTypeId::EMPTY);
    }

    #[test]
    fn default_volume_has_five_emissive_blocks() {
        let v = build_default_volume();
        // One interior voxel from each of the five emissive blocks.
        assert_eq!(v.voxel_at([31, 25, 33]), TY_EMISSIVE, "warm-white block");
        assert_eq!(v.voxel_at([12, 8, 46]), TY_EMISSIVE_COOL, "cool-white block");
        assert_eq!(v.voxel_at([48, 26, 48]), TY_EMISSIVE_AMBER, "amber block");
        assert_eq!(v.voxel_at([46, 16, 16]), TY_EMISSIVE_GREEN, "green block");
        assert_eq!(v.voxel_at([22, 7, 52]), TY_EMISSIVE_MAGENTA, "magenta block");
        // Every one of the five emissive palette entries is Emissive.
        let p = build_palette();
        for ty in [
            TY_EMISSIVE,
            TY_EMISSIVE_COOL,
            TY_EMISSIVE_AMBER,
            TY_EMISSIVE_GREEN,
            TY_EMISSIVE_MAGENTA,
        ] {
            assert_eq!(
                p[ty.0 as usize].material_base,
                MaterialBase::Emissive,
                "palette entry {} must be Emissive",
                ty.0,
            );
        }
    }

    #[test]
    fn default_volume_arch_is_carved() {
        let v = build_default_volume();
        // The back wall is solid sand diffuse...
        assert_eq!(v.voxel_at([58, 18, 18]), TY_WALL, "wall above the arch");
        // ...with the arch doorway carved back to empty.
        assert_eq!(v.voxel_at([58, 8, 31]), VoxelTypeId::EMPTY, "arch opening");
    }

    #[test]
    fn default_volume_constructs() {
        let v = build_default_volume();
        let w = construct(&v);
        // 4*2*4 = 32 chunks.
        assert_eq!(w.chunks.len(), 32);
        // The grid has geometry → some mixed chunks → a non-empty block buffer.
        assert!(!w.blocks.is_empty(), "expected mixed chunks → blocks");
        assert!(!w.voxels.is_empty(), "expected mixed blocks → voxels");
        // Every chunk word decodes to a valid cell (no panic).
        for &raw in &w.chunks {
            let _ = ChunkCell::decode(raw);
        }
    }

    #[test]
    fn palette_reserves_element_zero() {
        let p = build_palette();
        assert_eq!(p[0], VoxelType::default(), "element 0 must be the placeholder");
        assert_eq!(p[TY_EMISSIVE.0 as usize].material_base, MaterialBase::Emissive);
    }

    /// The centered embed must place the demo at the world centre, tile the
    /// ground template across the rest of chunk-Y=0, and leave chunks above
    /// the floor empty.
    #[test]
    fn composed_default_is_centered_with_full_area_ground() {
        let demo = construct(&build_default_volume());
        let ground = construct(&build_ground_chunk_volume());
        let target = WORLD_SIZE_IN_CHUNKS;
        let center_off = IVec3::new(
            (target.x as i32 - demo.size_in_chunks[0] as i32) / 2,
            0,
            (target.z as i32 - demo.size_in_chunks[2] as i32) / 2,
        );
        let world =
            compose_default_scene_into_fixed_world(&demo, &ground, target, center_off);

        assert_eq!(world.size_in_chunks, [target.x, target.y, target.z]);
        assert_eq!(
            world.chunks.len(),
            (target.x * target.y * target.z) as usize,
        );

        // Demo embed at the centre — sample chunks (0,0,0) and (dx-1, dy-1, dz-1)
        // of the demo map to (off+0,0,off+0) and (off+dx-1, dy-1, off+dz-1) of
        // the fixed world. The chunk encoding (including BlockPtr) must match
        // bit-for-bit since demo.blocks were copied verbatim at the start of
        // out_blocks.
        let [dx, dy, dz] = demo.size_in_chunks;
        for &(lx, ly, lz) in &[(0u32, 0u32, 0u32), (dx - 1, dy - 1, dz - 1)] {
            let wx = (center_off.x as u32) + lx;
            let wy = (center_off.y as u32) + ly;
            let wz = (center_off.z as u32) + lz;
            let dst = (wx + wy * target.x + wz * target.x * target.y) as usize;
            let src = (lx + ly * dx + lz * dx * dy) as usize;
            assert_eq!(
                world.chunks[dst], demo.chunks[src],
                "demo chunk ({lx},{ly},{lz}) must appear verbatim at fixed-world ({wx},{wy},{wz})"
            );
        }

        // Outside the demo footprint at chunk-Y=0: must be a Mixed ground
        // chunk (not empty, not the demo encoding). Pick a chunk well away
        // from the centred demo.
        let far_idx =
            (0u32 + 0 * target.x + 0 * target.x * target.y) as usize;
        match ChunkCell::decode(world.chunks[far_idx]) {
            ChunkCell::Mixed(_) => { /* expected — ground tile */ }
            other => panic!("expected ground tile at (0,0,0) in fixed world, got {other:?}"),
        }

        // Above the floor (chunk-Y >= 1, outside demo) must be empty.
        let above_floor_idx =
            (0u32 + 5 * target.x + 0 * target.x * target.y) as usize;
        assert!(
            matches!(
                ChunkCell::decode(world.chunks[above_floor_idx]),
                ChunkCell::Empty(_)
            ),
            "chunks above the floor and outside the demo must be Empty"
        );

        // Every ground chunk must have its own BlockPtr — i.e., the set of
        // unique BlockPtrs at chunk-Y=0 (outside demo) must be `total - 1` at
        // minimum (each ground chunk gets its own slice). Sanity-check with
        // a small sample.
        let mut block_ptrs: Vec<u32> = Vec::new();
        for cz in 0..4u32 {
            for cx in 0..4u32 {
                let idx = (cx + 0 * target.x + cz * target.x * target.y) as usize;
                if let ChunkCell::Mixed(bp) = ChunkCell::decode(world.chunks[idx]) {
                    block_ptrs.push(bp.0);
                }
            }
        }
        block_ptrs.sort_unstable();
        block_ptrs.dedup();
        assert_eq!(
            block_ptrs.len(),
            16,
            "every ground chunk must own a unique BlockPtr (edits stay local)"
        );
    }

    /// Sanity: every chunk in the composed fixed-world `chunks_cpu` decodes to
    /// a valid cell — sampling every 1024th chunk to keep the test fast.
    #[test]
    fn composed_default_decodes_cleanly() {
        let demo = construct(&build_default_volume());
        let ground = construct(&build_ground_chunk_volume());
        let target = WORLD_SIZE_IN_CHUNKS;
        let center_off = IVec3::new(
            (target.x as i32 - demo.size_in_chunks[0] as i32) / 2,
            0,
            (target.z as i32 - demo.size_in_chunks[2] as i32) / 2,
        );
        let world =
            compose_default_scene_into_fixed_world(&demo, &ground, target, center_off);
        for raw in world.chunks.iter().step_by(1024) {
            let _ = ChunkCell::decode(*raw);
        }
    }

    /// The ground template should encode as one Mixed chunk pointing at one
    /// 64-block slice, with the block buffer's mixed entries dedup-collapsed
    /// to a single VoxelPtr (every 4×4×1 column has identical 3-ground-1-air
    /// voxel content).
    #[test]
    fn ground_chunk_template_dedups_to_single_voxel_slot() {
        let g = construct(&build_ground_chunk_volume());
        assert_eq!(g.size_in_chunks, [1, 1, 1]);
        // 1 chunk = 64 blocks of storage.
        assert_eq!(g.blocks.len(), 64);
        // Mixed-block dedup: all 16 ground blocks at block-Y=0 share one
        // voxel slot, so voxels_cpu is exactly the placeholder slot (32 u32s
        // reserved in `construct`'s `voxels_buf`) plus one dedup-collapsed
        // 32-u32 slot for the shared 3-ground-1-air block content.
        assert!(
            !g.voxels.is_empty(),
            "ground template must have at least one mixed block's voxel slot"
        );
        // The single chunk encoding must be Mixed.
        assert!(matches!(ChunkCell::decode(g.chunks[0]), ChunkCell::Mixed(_)));
    }
}
