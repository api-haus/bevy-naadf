//! `streaming::noise_dispatch` — per-frame GPU dispatch wiring for the
//! streaming preset.
//!
//! Per `docs/orchestrate/streaming-world/02b-design-plan-b.md` §§ E + G:
//! Phase 2 routes the W5 stage-1 producer through a NEW shader
//! `noise_terrain.wgsl` instead of `generator_model.wgsl`. The output buffer
//! (`segment_voxel_buffer`) is byte-identical between the two shaders, so the
//! existing chunk_calc + bounds_calc chain runs over it unchanged.
//!
//! What this module owns:
//! - The Rust mirror of `NoiseTerrainParams` (the WGSL uniform block consumed
//!   by `noise_terrain.wgsl`), with static-asserted offsets so the WGSL
//!   `params.state` layout aligns with the Phase-1 `FnlState`.
//! - The shader-source helpers + bind-group layout descriptor + pipeline
//!   queue (mirrors `generator_model::*`).
//! - The combined-shader inliner (`build_noise_terrain_shader_src`), which
//!   concatenates `noise_fastnoiselite.wgsl` and `noise_terrain.wgsl` per the
//!   Phase-1 `build_oracle_dispatch_shader_src` precedent.
//!
//! The per-segment dispatch loop itself lives in
//! `render/construction/mod.rs`'s `naadf_gpu_producer_node` (a new
//! `streaming_mode_active` branch alongside the existing W5 + dense paths).
//! Reason: that node already has the right encoder + bind-group plumbing in
//! scope. Re-deriving it here would duplicate the W5 per-segment-submit
//! ordering fix (`mod.rs:2427-2453`).
//!
//! ## Extract-resource pattern
//!
//! The main-world `Residency` carries `admissions_this_frame` /
//! `evictions_this_frame`. The render-world mirrors these into a
//! [`StreamingExtractRender`] resource each frame via Bevy's `ExtractResource`.

use std::borrow::Cow;
use std::num::NonZeroU64;

use bevy::prelude::*;
use bevy::render::render_resource::{
    binding_types::{storage_buffer_sized, uniform_buffer_sized},
    BindGroupLayoutDescriptor, BindGroupLayoutEntries, BufferDescriptor, BufferUsages,
    CachedComputePipelineId, ComputePipelineDescriptor, PipelineCache, ShaderStages,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::shader::Shader;
use bytemuck::{Pod, Zeroable};

use super::noise_fastnoiselite::NOISE_FASTNOISELITE_SHADER_SRC;
use super::noise_fastnoiselite_cpu_oracle::FnlState;
use super::residency::{Residency, SlotIndex, WorldSegmentPos};

/// Inlined WGSL noise-terrain source — `include_str!` of
/// `assets/shaders/noise_terrain.wgsl`. Relative to this `.rs` file.
pub const NOISE_TERRAIN_SHADER_SRC: &str =
    include_str!("../assets/shaders/noise_terrain.wgsl");

/// Asset path of the WGSL noise-terrain shader (for the `AssetServer.load`
/// production path; Phase 2 may use either the inlined-string or the asset-
/// loader path).
pub const NOISE_TERRAIN_SHADER_PATH: &str = "shaders/noise_terrain.wgsl";

/// `@workgroup_size(4, 4, 4)` per `noise_terrain.wgsl`.
pub const NOISE_TERRAIN_WORKGROUP_SIZE: u32 = 4;

/// Build the combined shader source by inlining `noise_fastnoiselite.wgsl`
/// ABOVE the `noise_terrain.wgsl` body. Same pattern as
/// [`super::noise_fastnoiselite::build_oracle_dispatch_shader_src`].
pub fn build_noise_terrain_shader_src() -> String {
    let mut combined = String::with_capacity(
        NOISE_FASTNOISELITE_SHADER_SRC.len() + NOISE_TERRAIN_SHADER_SRC.len() + 256,
    );
    // Strip `#define_import_path` from the noise module so the combined
    // single-translation-unit doesn't carry the directive.
    for line in NOISE_FASTNOISELITE_SHADER_SRC.lines() {
        if line.trim_start().starts_with("#define_import_path") {
            combined.push_str("// (stripped #define_import_path for inlined compilation)\n");
            continue;
        }
        combined.push_str(line);
        combined.push('\n');
    }
    combined.push('\n');
    // Append everything after the `// @begin` marker in the terrain shader.
    let mut past_marker = false;
    for line in NOISE_TERRAIN_SHADER_SRC.lines() {
        if !past_marker {
            if line.trim_start().starts_with("// @begin") {
                past_marker = true;
            }
            continue;
        }
        combined.push_str(line);
        combined.push('\n');
    }
    if !past_marker {
        // Fallback — marker missing, concatenate the whole file.
        combined.push_str(NOISE_TERRAIN_SHADER_SRC);
    }
    combined
}

/// Rust mirror of `NoiseTerrainParams` in `noise_terrain.wgsl`.
///
/// Layout (std140-compatible uniform — `align = 16` because the embedded
/// `FnlState` host-shareable struct is 16-aligned in WGSL std140; even though
/// its scalar members are only 4-aligned, top-level host-shared structs round
/// up to 16):
/// - Row 0 (offset 0, 16 B): `seg_origin_in_voxels_{xyz}` (i32) +
///   `terrain_voxel_type_id` (u32).
/// - Row 1 (offset 16, 16 B): `group_size_in_chunks_x/y` (u32) +
///   `sea_level` (f32) + `terrain_amplitude` (f32).
/// - Rows 2..6 (offset 32, 80 B): `FnlState`.
///
/// Total size = 112 B (Rust + WGSL agree because `FnlState` contains only
/// scalars; no internal padding diff).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct NoiseTerrainParams {
    // Row 0
    pub seg_origin_in_voxels_x: i32,
    pub seg_origin_in_voxels_y: i32,
    pub seg_origin_in_voxels_z: i32,
    pub terrain_voxel_type_id: u32,
    // Row 1
    pub group_size_in_chunks_x: u32,
    pub group_size_in_chunks_y: u32,
    pub sea_level: f32,
    pub terrain_amplitude: f32,
    // Rows 2..6 — FnlState (80 B).
    pub state: FnlState,
}

// Compile-time layout pins. Total size = 32 B header + 80 B FnlState = 112 B.
const _: () = assert!(std::mem::size_of::<NoiseTerrainParams>() == 112);
const _: () = assert!(std::mem::align_of::<NoiseTerrainParams>() == 16);
const _: () = assert!(std::mem::offset_of!(NoiseTerrainParams, seg_origin_in_voxels_x) == 0);
const _: () = assert!(std::mem::offset_of!(NoiseTerrainParams, group_size_in_chunks_x) == 16);
const _: () = assert!(std::mem::offset_of!(NoiseTerrainParams, state) == 32);

/// Build the `noise_terrain_layout` bind-group-layout descriptor.
///
/// Bindings (`@group(0)`):
/// - 0: `chunk_data_rw` — `segment_voxel_buffer` (same buffer chunk_calc
///   consumes as read-only on its side).
/// - 1: `params` — [`NoiseTerrainParams`] uniform.
pub fn noise_terrain_layout_descriptor() -> BindGroupLayoutDescriptor {
    let params_size =
        NonZeroU64::new(std::mem::size_of::<NoiseTerrainParams>() as u64).unwrap();
    BindGroupLayoutDescriptor::new(
        "naadf_streaming_noise_terrain_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_sized(false, None),
                uniform_buffer_sized(false, Some(params_size)),
            ),
        ),
    )
}

/// Queue the `noise_terrain` pipeline against the given layout.
pub fn queue_noise_terrain_pipeline(
    asset_server: &AssetServer,
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
) -> CachedComputePipelineId {
    let shader = asset_server.load(NOISE_TERRAIN_SHADER_PATH);
    queue_noise_terrain_pipeline_with_handle(pipeline_cache, layout, shader)
}

/// Queue the `noise_terrain` pipeline against an already-resolved shader
/// handle. Used by the unit tests + the streaming pipeline init path that
/// builds the inlined shader source.
pub fn queue_noise_terrain_pipeline_with_handle(
    pipeline_cache: &PipelineCache,
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
) -> CachedComputePipelineId {
    pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("naadf_streaming_noise_terrain_pipeline".into()),
        layout: vec![layout],
        shader,
        entry_point: Some(Cow::from("fill_chunk_data_with_noise")),
        ..default()
    })
}

/// Allocate the `noise_terrain_params_buffer` (96 B uniform, rewritten in
/// place per admitted segment).
pub fn create_noise_terrain_params_buffer(
    device: &RenderDevice,
    queue: &RenderQueue,
) -> bevy::render::render_resource::Buffer {
    let buf = device.create_buffer(&BufferDescriptor {
        label: Some("naadf_streaming_noise_terrain_params"),
        size: std::mem::size_of::<NoiseTerrainParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let zeroed: NoiseTerrainParams = Zeroable::zeroed();
    queue.write_buffer(&buf, 0, bytemuck::bytes_of(&zeroed));
    buf
}

// -----------------------------------------------------------------------------
// Render-world extract resource — per-frame mirror of `Residency`'s per-frame
// admissions/evictions.
// -----------------------------------------------------------------------------

/// Main-world `Resource` holding the long-lived strong handle to the
/// inlined-source `noise_terrain` shader. Seeded once at startup by
/// [`seed_noise_terrain_shader`]; the render-world `ExtractSchedule` mirrors
/// the handle into [`StreamingExtractRender::noise_terrain_shader`] so
/// `prepare_construction` can pick it up and queue the pipeline lazily.
///
/// **Why on the main world?** `Assets<Shader>` lives in the main world; the
/// render-world `PipelineCache` reads it via the standard asset-server
/// extraction. Registering the inlined-source shader once at main-world
/// startup is the simplest path to a stable handle (Bevy's render-world
/// `Assets<Shader>` isn't reliably reachable from `init_gpu_resource`
/// callbacks).
#[derive(Resource, Clone)]
pub struct StreamingShaderHandle(pub Handle<Shader>);

/// `Startup` system — register the inlined-source `noise_terrain` shader
/// (`build_noise_terrain_shader_src`) as an `Assets<Shader>` asset and stash
/// the strong handle in [`StreamingShaderHandle`]. Wired by
/// [`super::StreamingPlugin::build`].
pub fn seed_noise_terrain_shader(
    mut commands: Commands,
    mut shaders: ResMut<bevy::asset::Assets<bevy::shader::Shader>>,
) {
    let combined = build_noise_terrain_shader_src();
    let shader = bevy::shader::Shader::from_wgsl(
        combined,
        "shaders/noise_terrain_combined.wgsl",
    );
    let handle = shaders.add(shader);
    commands.insert_resource(StreamingShaderHandle(handle));
}

/// Render-world mirror of the main-world [`Residency`] per-frame deltas.
///
/// `ExtractSchedule` writes this from the main-world residency resource each
/// frame; the streaming-mode branch of `naadf_gpu_producer_node` consumes it.
#[derive(Resource, Clone, Debug)]
pub struct StreamingExtractRender {
    /// `true` when the main world is running the `ProceduralStreaming` preset.
    /// Off otherwise.
    pub streaming_mode_active: bool,
    /// `true` when the main world is running the `ProceduralStatic` preset
    /// (Phase 2.4 — independent of `streaming_mode_active`; the two are
    /// mutually exclusive). Drives the one-shot full-world dispatch branch
    /// in `naadf_gpu_producer_node`.
    pub static_mode_active: bool,
    /// Per-frame admissions (camera-distance-sorted, capped at
    /// `--max-segments-per-frame`).
    pub admissions_this_frame: Vec<(WorldSegmentPos, SlotIndex)>,
    /// Per-frame evictions.
    pub evictions_this_frame: Vec<SlotIndex>,
    /// Camera-anchored sea level + amplitude + noise state — copied from the
    /// main-world [`super::chunk_source::NoiseChunkSource`].
    pub noise_state: FnlState,
    pub sea_level: f32,
    pub terrain_amplitude: f32,
    pub solid_voxel_type_id: u32,
    /// Mirror of the main-world [`StreamingShaderHandle`] — the inlined-source
    /// `noise_terrain` shader handle. `prepare_construction` queues the
    /// pipeline lazily once this becomes `Some`.
    pub noise_terrain_shader: Option<Handle<Shader>>,
    /// streaming-world Phase 2.6 — flat copy of
    /// [`Residency::window`]`::indirection_buffer()`. 512 u32s.
    /// [`upload_window_indirection`] copies this into the GPU buffer at
    /// `Render::Queue`.
    pub window_indirection: Vec<u32>,
    /// streaming-world Phase 2.6 — the live residency origin in segments.
    /// Used by the streaming dispatch loop to compute window-local positions
    /// (`local = world_seg - origin`) without needing to know slot indices.
    pub window_origin: bevy::math::IVec3,
    /// streaming-world Phase 2.11
    /// (`docs/orchestrate/streaming-world/03n-diagnosis-aadf-building.md`
    /// punch-list item 1) — mirrors `Residency::is_cold_start_complete()`.
    /// `true` once every slot in the window has been admitted at least once.
    /// Gates the W3 regime-1 seed in `prepare_construction` so the seed
    /// fires AFTER all chunks_buffer slots carry real chunk_calc-produced
    /// state — prevents the W3 chain from baking stale long-skip AADFs
    /// through yet-to-be-admitted zero-chunks (the bug in `03n` § Root
    /// cause). Steady-state boundary crossings drop this to false until
    /// the new admissions complete; the W3 seed only re-runs in scoped form
    /// via the per-admission re-seed (Phase 2.11 punch-list item 2) once
    /// cold-start is again complete.
    pub cold_start_complete: bool,
    /// streaming-world Phase 2.11
    /// (`03n-diagnosis-aadf-building.md` punch-list item 2) — `true` when
    /// this frame triggered any origin shift (evictions > 0 or
    /// admissions > 0). Drives the render-world W3 full-world re-seed
    /// dispatch.
    ///
    /// Why full-world re-seed (not scoped): the chunks_buffer is slot-
    /// indexed; AADFs are stored per slot. When origin shifts, the
    /// indirection table rebinds (slot S now at a different window-local
    /// position), but slot S's chunks_buffer data does NOT move. The
    /// AADFs in slot S's chunks describe neighbour relationships at
    /// PRE-SHIFT window-local coords. The renderer + bounds-chain
    /// interpret them via POST-SHIFT indirection → neighbour-via-indirection
    /// now resolves to a different slot, so the AADF skip distance may now
    /// "lie" (skip past terrain that is now adjacent in window-local space).
    ///
    /// Scope-aware re-seed (Phase 2.11 punch-list item 2's original
    /// proposal) covers ONLY the just-admitted segments + a 1-group
    /// border. That's too narrow: AADFs up to 31 chunks long can lie
    /// about chunks far from any admission boundary. The full-world
    /// re-seed (mark every group's mask bit, re-enqueue all 32768 groups
    /// at size 0) is the simple correct fix. Cost: the W3 regime-2
    /// background chain re-converges over ~30 frames at one bound size
    /// per round; user-visible during shift bursts, invisible at
    /// steady-state.
    pub w3_reseed_full_world: bool,
    /// streaming-world Phase 2.12
    /// (`docs/orchestrate/streaming-world/02e-design-phase-2-12.md` § B,
    /// MUST-1) — mirror of `Residency::clear_on_bind_queue`: slot indices
    /// whose binding changed this frame (newly bound in Pass 3). The
    /// render-world `clear_streaming_bound_slots` system drains this list
    /// at `Render::Queue` time, issuing one `clear_buffer` per slot on a
    /// single command encoder. Each `clear_buffer` zeroes 32 KiB
    /// (`CHUNKS_PER_SLOT * 8` = 4096 chunks × `vec2<u32>`). Cost:
    /// ~50 us per slot × up to 32 slots per shift = ~1.6 ms on shift
    /// frames; zero on steady-state non-shift frames.
    pub clear_on_bind_slots: Vec<SlotIndex>,
}

impl Default for StreamingExtractRender {
    fn default() -> Self {
        // `FnlState` is `Pod + Zeroable` — a zeroed state is a valid empty
        // configuration. The `streaming_mode_active = false` discriminator
        // means consumers never read these defaults in practice.
        Self {
            streaming_mode_active: false,
            static_mode_active: false,
            admissions_this_frame: Vec::new(),
            evictions_this_frame: Vec::new(),
            noise_state: Zeroable::zeroed(),
            sea_level: 0.0,
            terrain_amplitude: 1.0,
            solid_voxel_type_id: 1,
            noise_terrain_shader: None,
            window_indirection: Vec::new(),
            window_origin: bevy::math::IVec3::ZERO,
            cold_start_complete: false,
            w3_reseed_full_world: false,
            clear_on_bind_slots: Vec::new(),
        }
    }
}

/// `ExtractSchedule` system — mirror the main-world residency state into the
/// render-world [`StreamingExtractRender`] resource. Wired by the
/// [`super::StreamingPlugin`].
///
/// Phase 2.4 — also mirrors the `ProceduralStaticActive` marker resource so
/// the render-world picks up the static preset's one-shot dispatch branch.
/// The streaming and static branches are mutually exclusive (the install
/// paths insert exactly one of `Residency` / `ProceduralStaticActive` —
/// never both).
/// streaming-world Phase 2.12 (`02e-design-phase-2-12.md` § B) — static
/// cross-world accumulator for pending clear-on-bind slot ids. Extract
/// APPENDS to it from the main world; the render-world
/// `clear_streaming_bound_slots` system DRAINS it once `WorldGpu` is
/// available and the per-slot `clear_buffer` GPU command has been
/// recorded. This pattern survives the Frame-0 race where `WorldGpu`
/// isn't yet allocated by `prepare_world_gpu` (a build-once
/// `PrepareResources` system that may take 1-3 frames). A naive
/// `Vec<SlotIndex>` mirror on `StreamingExtractRender` would silently
/// drop the cold-start binds during that race window.
pub static PENDING_CLEAR_ON_BIND_SLOTS: std::sync::Mutex<Vec<SlotIndex>> =
    std::sync::Mutex::new(Vec::new());

pub fn extract_streaming_state(
    mut commands: Commands,
    main_world: ResMut<bevy::render::MainWorld>,
) {
    // streaming-world Phase 2.12 — `ResMut<MainWorld>` so the extract can
    // DRAIN `Residency::clear_on_bind_queue` atomically into the cross-
    // world `PENDING_CLEAR_ON_BIND_SLOTS` accumulator. The accumulator
    // outlives any single frame, so a Frame-0 race where `WorldGpu`
    // isn't yet allocated does NOT lose binds — they stay in
    // `PENDING_CLEAR_ON_BIND_SLOTS` until the render system drains them.
    // Mirrors the `extract_world_changes` ResMut<MainWorld> pattern at
    // `render/construction/mod.rs:815-832`.
    let main_world: &mut bevy::ecs::world::World = &mut **main_world.into_inner();

    let shader = main_world
        .get_resource::<StreamingShaderHandle>()
        .map(|s| s.0.clone());
    let static_active = main_world
        .get_resource::<super::chunk_source::ProceduralStaticActive>()
        .is_some();
    if static_active {
        // Static preset path: NoiseChunkSource is the only required companion;
        // there is no Residency.
        let Some(chunk_source) =
            main_world.get_resource::<super::chunk_source::NoiseChunkSource>()
        else {
            // NoiseChunkSource missing — should never happen if the install
            // path ran. Emit a default-ish state with the shader handle so
            // the pipeline-queue path can still progress.
            let mut default_state = StreamingExtractRender::default();
            default_state.noise_terrain_shader = shader;
            commands.insert_resource(default_state);
            return;
        };
        commands.insert_resource(StreamingExtractRender {
            streaming_mode_active: false,
            static_mode_active: true,
            admissions_this_frame: Vec::new(),
            evictions_this_frame: Vec::new(),
            noise_state: chunk_source.state,
            sea_level: chunk_source.sea_level,
            terrain_amplitude: chunk_source.terrain_amplitude,
            solid_voxel_type_id: chunk_source.solid_voxel_type_id,
            noise_terrain_shader: shader,
            // Static preset never uses the indirection table — it writes at
            // absolute world chunk coords via the flat-coord layout.
            window_indirection: Vec::new(),
            window_origin: bevy::math::IVec3::ZERO,
            // Static preset: not streaming, so the W3 cold-start gate is
            // irrelevant; the static branch flips `bounds_initialized = true`
            // directly. Set to false; the streaming-only W3 seed gate
            // (`!streaming_active` branch) doesn't consume this field.
            cold_start_complete: false,
            w3_reseed_full_world: false,
            clear_on_bind_slots: Vec::new(),
        });
        return;
    }
    // Read main-world NoiseChunkSource (read-only, optional).
    let chunk_source_data = main_world
        .get_resource::<super::chunk_source::NoiseChunkSource>()
        .map(|c| {
            (
                c.state,
                c.sea_level,
                c.terrain_amplitude,
                c.solid_voxel_type_id,
            )
        });

    // Now take the mutable Residency borrow (needed to DRAIN the
    // clear-on-bind queue atomically with extract — see § B above).
    let Some(mut residency) = main_world.get_resource_mut::<Residency>() else {
        // Even when streaming isn't active, propagate the shader handle so
        // the lazy queue path picks it up the moment streaming flips on.
        let mut default_state = StreamingExtractRender::default();
        default_state.noise_terrain_shader = shader;
        commands.insert_resource(default_state);
        return;
    };
    let Some((noise_state, sea_level, terrain_amplitude, solid_voxel_type_id)) =
        chunk_source_data
    else {
        let mut default_state = StreamingExtractRender::default();
        default_state.noise_terrain_shader = shader;
        commands.insert_resource(default_state);
        return;
    };
    // streaming-world Phase 2.11 — flag a full-world W3 re-seed when this
    // frame had any origin shift (admissions or evictions). The chunks_buffer
    // is slot-indexed; AADFs are interpreted via the indirection table.
    // Origin shifts rebind indirection without moving the chunks_buffer data,
    // so all slot-stored AADFs become stale (they describe neighbours at
    // PRE-SHIFT window-local positions; the renderer now resolves neighbours
    // via POST-SHIFT indirection → potentially different slots with
    // potentially-solid content). Simple correct fix: full-world re-seed
    // (re-enqueues all 32768 groups at bound_size 0; the chain reconverges
    // over ~30 frames at 1 round per axis-size combo). Gated on
    // `cold_start_complete` so cold-start admissions don't repeatedly fire
    // the seed.
    let w3_reseed_full_world = !residency.admissions_this_frame.is_empty()
        || !residency.evictions_this_frame.is_empty();

    // Phase 2.12 (`02e-design-phase-2-12.md` § B) — DRAIN the main-world
    // `clear_on_bind_queue` into the cross-world `PENDING_CLEAR_ON_BIND_SLOTS`
    // accumulator. `std::mem::take` moves the Vec out of main-world residency
    // (replacing it with an empty Vec); the freed slot-id list is appended to
    // the accumulator which the render-world drains once `WorldGpu` is ready.
    // This survives the Frame-0 race where `WorldGpu` isn't yet allocated by
    // the build-once `prepare_world_gpu` system.
    let drained_from_main = std::mem::take(&mut residency.clear_on_bind_queue);
    if !drained_from_main.is_empty() {
        if let Ok(mut acc) = PENDING_CLEAR_ON_BIND_SLOTS.lock() {
            acc.extend(drained_from_main);
        }
    }

    commands.insert_resource(StreamingExtractRender {
        streaming_mode_active: true,
        static_mode_active: false,
        admissions_this_frame: residency.admissions_this_frame.clone(),
        evictions_this_frame: residency.evictions_this_frame.clone(),
        noise_state,
        sea_level,
        terrain_amplitude,
        solid_voxel_type_id,
        noise_terrain_shader: shader,
        // Phase 2.6 — copy the live indirection table so
        // `upload_window_indirection` (Render::Queue) can write it to GPU.
        // 2 KB clone per frame — cheap.
        window_indirection: residency.window.indirection_buffer().to_vec(),
        window_origin: residency.window.origin(),
        cold_start_complete: residency.is_cold_start_complete(),
        w3_reseed_full_world,
        // Phase 2.12 — the `clear_on_bind_slots` field on
        // `StreamingExtractRender` is retained for compatibility but is now
        // populated EMPTY. The render-world clear system reads
        // `PENDING_CLEAR_ON_BIND_SLOTS` directly (the cross-world
        // accumulator survives the Frame-0 `WorldGpu`-not-ready race).
        clear_on_bind_slots: Vec::new(),
    });
}


// -----------------------------------------------------------------------------
// Phase 2.6 — per-frame GPU upload of the window indirection table.
// -----------------------------------------------------------------------------

/// Render-app `Render::Queue` system — write the
/// [`StreamingExtractRender::window_indirection`] bytes into the GPU buffer
/// `ConstructionGpu::window_indirection_buffer` (allocated in
/// `prepare_construction` on the first streaming-active frame).
///
/// Runs in `Render::Queue` so the write_buffer call happens BEFORE the
/// producer node consumes the bind group (the renderer-side world bind
/// group binds the same buffer at `@group(0) @binding(8)`).
///
/// Per `docs/orchestrate/streaming-world/02c-design-windowed-slot-map.md` § D6
/// — dedicated upload system, separate from `prepare_construction`'s
/// allocation site.
pub fn upload_window_indirection(
    gpu: Option<bevy::prelude::Res<crate::render::construction::ConstructionGpu>>,
    streaming_extract: Option<bevy::prelude::Res<StreamingExtractRender>>,
    render_queue: bevy::prelude::Res<RenderQueue>,
) {
    let (Some(gpu), Some(s)) = (gpu, streaming_extract) else {
        return;
    };
    if !s.streaming_mode_active {
        return;
    }
    if s.window_indirection.is_empty() {
        return;
    }
    let Some(buf) = gpu.window_indirection_buffer.as_ref() else {
        return;
    };
    render_queue.write_buffer(buf, 0, bytemuck::cast_slice(&s.window_indirection));
}

// -----------------------------------------------------------------------------
// streaming-world Phase 2.12 — clear-on-bind chunks_buffer system
// (`docs/orchestrate/streaming-world/02e-design-phase-2-12.md` § B, MUST-1).
// -----------------------------------------------------------------------------

/// Render-app `Render::Queue` system — zero a slot's `chunks_buffer` region
/// the same frame `residency_driver`'s Pass 3 rebound that slot.
///
/// **Why this exists**: when origin shifts, up to 32 slots are evicted +
/// rebound to new world segments. `residency_driver` writes the new
/// indirection entries SYNCHRONOUSLY (`windowed_slot_map.rs::bind()`), but
/// the per-admission producer node only processes `max_segments_per_frame = 4`
/// admissions per frame. For the other ~28 rebound slots, the indirection
/// table points the NEW window-local positions at slots whose `chunks_buffer`
/// region STILL CONTAINS the previously-evicted segment's data. The renderer
/// + W3 bounds chain read this stale data as if it were current — producing
/// the "ghost of old terrain" visual corruption diagnosed in
/// `03p-diagnosis-remaining-bugs.md` § Bug 1.
///
/// **The fix**: every slot in `StreamingExtractRender::clear_on_bind_slots`
/// has its `chunks_buffer` region zeroed on a single command encoder before
/// the producer node runs. Post-clear, the chunks decode as `state = 0 >>
/// 30 = UNIFORM_EMPTY, AADFs = 0` — readers see UNIFORM_EMPTY (sky) for
/// un-admitted-yet slots, NOT ghost data.
///
/// **Cost**: ~50 us per slot × up to 32 slots per shift = ~1.6 ms on shift
/// frames; zero on steady-state non-shift frames (the list is empty).
///
/// **Ordering**: runs in `Render::Queue` (same set as
/// [`upload_window_indirection`]). Both must run BEFORE the producer node
/// (which lives in `Core3d::PostProcess`). Order between the two doesn't
/// matter — both write to GPU storage buffers; the renderer's first
/// chunks-buffer read happens at the start of the render-graph pass, well
/// after `Render::Queue`.
///
/// The per-admission `clear_buffer` call at `mod.rs:3301-3305` (Phase 2.11
/// item 3) is RETAINED as defensive code — for the 4-of-32 slots that
/// ALSO admit this frame, both clears fire (idempotent on zero data;
/// wgpu auto-merges the COPY-DST→COPY-DST barriers).
pub fn clear_streaming_bound_slots(
    world_gpu: Option<bevy::prelude::Res<crate::render::prepare::WorldGpu>>,
    streaming_extract: Option<bevy::prelude::Res<StreamingExtractRender>>,
    render_device: bevy::prelude::Res<bevy::render::renderer::RenderDevice>,
    render_queue: bevy::prelude::Res<RenderQueue>,
) {
    let (Some(world_gpu), Some(s)) = (world_gpu, streaming_extract) else {
        // Either `WorldGpu` not yet allocated by `prepare_world_gpu` (build-
        // once system; takes 1-3 frames on cold-start) or the
        // `StreamingExtractRender` resource isn't yet present (extract hasn't
        // run for the first time). Bail; the slot ids stay in
        // `PENDING_CLEAR_ON_BIND_SLOTS` for the next frame.
        return;
    };
    if !s.streaming_mode_active {
        return;
    }
    // Drain the cross-world accumulator. Survives Frame-0 races where
    // `WorldGpu` was unavailable.
    let pending: Vec<SlotIndex> = match PENDING_CLEAR_ON_BIND_SLOTS.lock() {
        Ok(mut acc) => std::mem::take(&mut *acc),
        Err(_) => return,
    };
    if pending.is_empty() {
        return;
    }
    /// 4096 chunks per slot (16×16×16 chunks per segment).
    const CHUNKS_PER_SLOT: u32 = 4096;
    /// `vec2<u32>` = 8 bytes per chunk_pair.
    const CHUNK_PAIR_BYTES: u64 = 8;
    let slot_size_bytes = (CHUNKS_PER_SLOT as u64) * CHUNK_PAIR_BYTES;
    let mut enc = render_device.create_command_encoder(
        &bevy::render::render_resource::CommandEncoderDescriptor {
            label: Some("naadf_streaming_clear_bound_slots_encoder"),
        },
    );
    for slot in &pending {
        let slot_offset_bytes = (slot.0 as u64) * slot_size_bytes;
        enc.clear_buffer(
            &world_gpu.chunks_buffer,
            slot_offset_bytes,
            Some(slot_size_bytes),
        );
    }
    render_queue.submit([enc.finish()]);
    bevy::log::debug!(
        "streaming-world Phase 2.12: cleared {} chunks_buffer slot region(s) \
         (clear-on-bind)",
        pending.len(),
    );
}

/// Build the [`NoiseTerrainParams`] for one segment admission.
pub fn build_noise_terrain_params(
    seg_origin_in_voxels: IVec3,
    state: &StreamingExtractRender,
    segment_chunks: u32,
) -> NoiseTerrainParams {
    NoiseTerrainParams {
        seg_origin_in_voxels_x: seg_origin_in_voxels.x,
        seg_origin_in_voxels_y: seg_origin_in_voxels.y,
        seg_origin_in_voxels_z: seg_origin_in_voxels.z,
        terrain_voxel_type_id: state.solid_voxel_type_id,
        group_size_in_chunks_x: segment_chunks,
        group_size_in_chunks_y: segment_chunks,
        sea_level: state.sea_level,
        terrain_amplitude: state.terrain_amplitude,
        state: state.noise_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_terrain_params_layout() {
        assert_eq!(std::mem::size_of::<NoiseTerrainParams>(), 112);
        assert_eq!(std::mem::align_of::<NoiseTerrainParams>(), 16);
        // Sanity-check Row 0 fields:
        assert_eq!(std::mem::offset_of!(NoiseTerrainParams, seg_origin_in_voxels_x), 0);
        assert_eq!(std::mem::offset_of!(NoiseTerrainParams, terrain_voxel_type_id), 12);
        // Row 1
        assert_eq!(std::mem::offset_of!(NoiseTerrainParams, group_size_in_chunks_x), 16);
        assert_eq!(std::mem::offset_of!(NoiseTerrainParams, sea_level), 24);
        // FnlState starts at row 2.
        assert_eq!(std::mem::offset_of!(NoiseTerrainParams, state), 32);
    }

    #[test]
    fn shader_inliner_strips_directive_and_finds_marker() {
        let src = build_noise_terrain_shader_src();
        assert!(src.contains("fn fnl_get_noise_3d"), "noise module not inlined");
        assert!(
            src.contains("fn fill_chunk_data_with_noise"),
            "noise_terrain entry point missing"
        );
        let has_directive = src
            .lines()
            .any(|line| line.trim_start().starts_with("#define_import_path"));
        assert!(!has_directive, "#define_import_path leaked into combined");
        let has_marker = src.lines().any(|line| line.trim_start().starts_with("// @begin"));
        assert!(!has_marker, "// @begin marker leaked");
    }
}
