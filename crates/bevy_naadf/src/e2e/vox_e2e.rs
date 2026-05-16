//! `--vox-e2e` mode — automated regression test for the Track A `.vox` load
//! path (`docs/orchestrate/feature-completeness/03a-impl-vox-loading.md`).
//!
//! ## Goal
//!
//! Land an automated end-to-end gate that:
//!   1. Synthesises a small `.vox` file **in memory** (no checked-in binary
//!      fixture per `03a-impl-vox-loading.md` Deviation 1) with **≥ 2 models
//!      under non-trivial `nTRN` translations** so the scene-graph composition
//!      fix from `03a-followup-empty-scene-diagnosis.md` is exercised.
//!   2. Writes the bytes to a deterministic temp file so the production
//!      `--vox <path>` path (`crates/bevy_naadf/src/main.rs:21-33`) drives the
//!      load — same code path the user invokes manually.
//!   3. Boots the existing windowed e2e harness ([`crate::e2e::run_with_app`])
//!      with [`crate::GridPreset::Vox`] selected, so the same fixed-pose
//!      camera, frame budget, readback, and `PipelineCache` scan run.
//!   4. Replaces the default-scene gates (which sample rectangles tuned for
//!      the hard-coded `voxel/grid.rs::build_default_volume` content layout)
//!      with a "non-skybox" assertion: a region the synthesised geometry
//!      projects into must have luminance meaningfully different from the
//!      atmosphere-tinted sky band.
//!
//! ## Why an in-memory synthesised fixture
//!
//! Three reasons mirror the Track A implementer's Deviation 1 reasoning
//! (`03a-impl-vox-loading.md` §7 Deviation 1):
//!   - **No binary blobs in the tree.** Same drift-risk concern Track A
//!     already settled with `DotVoxData::write_vox` → `parse_vox_bytes`
//!     in-process round-tripping.
//!   - **Sized to fit the `MAX_CHUNKS_PER_AXIS = 32` cap** the 03a-followup
//!     lowered after scene-graph composition landed. Big real-world `.vox`
//!     files (e.g. `Oasis_Hard_Cover.vox` at 93 × 34 × 84 chunks) explicitly
//!     cannot be the test fixture.
//!   - **Reuses the production `--vox` path verbatim**: the bytes go
//!     `DotVoxData::write_vox → std::fs::write → load_vox → parse_vox_bytes
//!     → flatten_scene → build_world_from_vox`, exercising
//!     [`crate::voxel::vox_import`]'s full surface area.
//!
//! ## Fixture geometry
//!
//! Two emissive models, both of palette index 1 (slot 1 is given a
//! `MATL { _emit: 1.0 }` so it renders bright pre-GI — the emissive
//! contribution lands in `color_layered.x = emit * (1+flux)² * 5 = 5.0`
//! per [`crate::voxel::vox_import::vox_palette_to_voxel_types`]).
//!
//! The two models are sized + translated so the composed NAADF world AABB
//! matches the default-scene `64 × 32 × 64` voxel volume — the same size
//! the e2e camera pose (`gates::e2e_camera_transform` at NAADF
//! `(86, 42, 90)` looking at `(32, 16, 32)`) was calibrated against in
//! `02-research.md` / `gates.rs:23-50`:
//!
//!   - **Model A — emissive ground slab.** MV size `60 × 60 × 4`,
//!     `nTRN _t = "30 30 2"`. Centered local origin `(-30, -30, -2)` puts
//!     the slab at MV `(0..59, 0..59, 0..3)`. After Z↔Y swap → NAADF
//!     `(0..59, 0..3, 0..59)` — the ground plane covers the floor of
//!     the world.
//!   - **Model B — central emissive tower.** MV size `20 × 20 × 28`,
//!     `nTRN _t = "30 30 16"`. Centered local origin `(-10, -10, -14)`
//!     puts the tower at MV `(20..39, 20..39, 2..29)`. After Z↔Y swap →
//!     NAADF `(20..39, 2..29, 20..39)` — a tall lit cuboid sitting on
//!     top of the slab, directly under the e2e camera's look target
//!     `(32, 16, 32)`.
//!
//! The two `_t` translations are non-trivial AND differ along the MV-z
//! axis (`_t.z = 2` vs `_t.z = 16`) — the up-axis composition is exactly
//! the seam the 03a-followup identity-only walk regression would have
//! destroyed. A regression that stacked both models at origin would
//! either:
//!   - collapse the world AABB so the camera spawns inside opaque
//!     material (symptom-1 from `03a-followup`), OR
//!   - shrink the lit cuboid to a single overlap region that no longer
//!     covers the central screen rect the [`assert_vox_geometry_visible`]
//!     gate samples.
//!
//! Combined MV AABB: `x ∈ (0..59)`, `y ∈ (0..59)`, `z ∈ (0..29)` → MV size
//! `(60, 60, 30)`. After Z↔Y swap NAADF size `(60, 30, 60)`, rounded up to
//! chunks `(4, 2, 4)` = `64 × 32 × 64` voxels. Comfortably within the
//! 03a-followup `MAX_CHUNKS_PER_AXIS = 32` cap.
//!
//! ## Gate
//!
//! [`assert_vox_geometry_visible`] samples a centred screen rectangle and
//! asserts its mean luminance is meaningfully above the no-geometry sky
//! luminance band. Cited baseline numbers (from the post-Track-A run
//! recorded in `03a-impl-vox-loading.md` Verification):
//!   - default-scene emissive luminance ≈ **247**
//!   - default-scene solid (GI-lit diffuse) ≈ **242**
//!   - default-scene sky luminance ≈ **146**
//! These come from [`crate::e2e::gates::region_luminance_report`] which the
//! standard driver prints every run; the same helper is used here to log
//! the vox-e2e regions for diagnostics. The vox-e2e gate threshold is the
//! upper sky-band boundary (`> SKY_LUMINANCE_CEILING`) — a region inside
//! the synthesised emissive geometry should sit *substantially* above the
//! sky band; if the load silently rendered no geometry, the region would
//! collapse to the sky luminance (~146) or below.

use std::path::{Path, PathBuf};

use bevy::prelude::AppExit;
use std::collections::HashMap as StdHashMap;
use std::io::Cursor;

use crate::e2e::framebuffer::{Framebuffer, Rect};

/// The screen-rect fractional bounds the [`assert_vox_geometry_visible`]
/// gate samples — a central 40% × 40% region that the synthesised emissive
/// geometry projects into at the fixed e2e camera pose. Wider than the
/// default-scene gates (which sample 10% × 10% slivers tuned for specific
/// voxels) because the synthesised fixture fills the central frame area
/// with emissive material — a narrow rect risks landing on a boundary.
const VOX_GEOMETRY_RECT_FRACS: (f32, f32, f32, f32) = (0.30, 0.30, 0.70, 0.70);

/// Mean luminance ceiling below which the centre region is considered
/// "skybox-or-darker". Calibrated against the baseline figures cited in
/// `crates/bevy_naadf/src/e2e/vox_e2e.rs` module docs and
/// `03a-impl-vox-loading.md` Verification (default-scene sky luminance ≈
/// 146). Set to **160.0** — above the measured sky band (~146) so a
/// non-skybox capture must exceed the atmosphere-tint floor, but well
/// below the lit-emissive range (~240+) so the gate has comfortable
/// headroom and does not flap on minor framebuffer noise.
///
/// **Defensible rationale**: this is a "non-skybox" gate, not a tight
/// brightness match. The lower bound is set just above the measured sky
/// band so a frame that is "mostly sky everywhere" fails, and a frame that
/// has emissive geometry covering the centre passes by a wide margin.
const SKY_LUMINANCE_CEILING: f32 = 160.0;

/// Build the synthesised multi-model `.vox` fixture as parsed `DotVoxData`.
///
/// Two 12 × 12 × 12 emissive cubes referenced by separate `nSHP` / `nTRN`
/// pairs under one `nGRP` root. Translations are non-trivial so a
/// scene-graph composition regression manifests as cubes stacked at
/// origin.
pub fn build_vox_e2e_fixture() -> dot_vox::DotVoxData {
    // Slot 1 is the emissive palette index — voxels write `i = 1`, which
    // `vox_import.rs` shifts to `VoxelTypeId(2)`. `MATL { _emit = 1.0 }`
    // on that slot lands `MaterialBase::Emissive` per
    // `vox_palette_to_voxel_types` (`voxel/vox_import.rs:758-811`).
    let mut materials: Vec<dot_vox::Material> = (0..256)
        .map(|i| dot_vox::Material {
            id: i,
            properties: {
                let mut d: dot_vox::Dict =
                    StdHashMap::new().into_iter().collect();
                d.insert("_type".to_owned(), "_diffuse".to_owned());
                d
            },
        })
        .collect();
    for m in &mut materials {
        if m.id == 1 {
            m.properties.insert("_type".into(), "_emit".into());
            // emission = 1.0 * (1+0)^2 * 5 = 5.0 → color_layered.x = 5.0
            // in linear units; the warm-white default palette colour
            // (slot 1 in DEFAULT_PALETTE) lands as the base albedo.
            m.properties.insert("_emit".into(), "1.0".into());
            m.properties.insert("_flux".into(), "0.0".into());
        }
    }

    // --- Model A — emissive ground slab (MV 60 × 60 × 4 of palette 1). -------
    //
    // Translated via `nTRN _t = "30 30 2"` to MV (0..59, 0..59, 0..3);
    // after Z↔Y swap NAADF (0..59, 0..3, 0..59) — the ground plane.
    //
    // Each model's voxels are written in MagicaVoxel local coords
    // `0..size`; the `dot_vox::Voxel { x, y, z, i }` fields are `u8` so
    // sizes ≤ 256 fit. Slot 1 is the emissive palette index.
    let slab_size = dot_vox::Size { x: 60, y: 60, z: 4 };
    let slab_voxels: Vec<dot_vox::Voxel> = {
        let mut v = Vec::with_capacity(60 * 60 * 4);
        for z in 0..4u8 {
            for y in 0..60u8 {
                for x in 0..60u8 {
                    v.push(dot_vox::Voxel { x, y, z, i: 1 });
                }
            }
        }
        v
    };

    // --- Model B — central emissive tower (MV 20 × 20 × 28). -----------------
    //
    // Translated via `nTRN _t = "30 30 16"` to MV (20..39, 20..39, 2..29);
    // after Z↔Y swap NAADF (20..39, 2..29, 20..39) — a lit cuboid sitting
    // on top of the slab, directly under the e2e camera's look target.
    let tower_size = dot_vox::Size { x: 20, y: 20, z: 28 };
    let tower_voxels: Vec<dot_vox::Voxel> = {
        let mut v = Vec::with_capacity(20 * 20 * 28);
        for z in 0..28u8 {
            for y in 0..20u8 {
                for x in 0..20u8 {
                    v.push(dot_vox::Voxel { x, y, z, i: 1 });
                }
            }
        }
        v
    };

    let models = vec![
        dot_vox::Model {
            size: slab_size,
            voxels: slab_voxels,
        },
        dot_vox::Model {
            size: tower_size,
            voxels: tower_voxels,
        },
    ];

    // Scene graph: root `nTRN` (identity) → root `nGRP` (two children) →
    // two `nTRN/nSHP` pairs, each translating one model to a distinct MV
    // position. Both translations differ along the MV-z axis (the up
    // axis), so the scene-graph composition fix lifted by
    // `03a-followup-empty-scene-diagnosis.md` is directly exercised.
    let scenes = vec![
        // 0 — root nTRN under identity.
        dot_vox::SceneNode::Transform {
            attributes: dict_empty(),
            frames: vec![dot_vox::Frame::new(dict_empty())],
            child: 1,
            layer_id: 0,
        },
        // 1 — root group: two children.
        dot_vox::SceneNode::Group {
            attributes: dict_empty(),
            children: vec![2, 4],
        },
        // 2 — nTRN: slab translated to MV `_t = (30, 30, 2)`.
        dot_vox::SceneNode::Transform {
            attributes: dict_empty(),
            frames: vec![dot_vox::Frame::new(dict_with("_t", "30 30 2"))],
            child: 3,
            layer_id: 0,
        },
        // 3 — nSHP referencing the slab.
        dot_vox::SceneNode::Shape {
            attributes: dict_empty(),
            models: vec![dot_vox::ShapeModel {
                model_id: 0,
                attributes: dict_empty(),
            }],
        },
        // 4 — nTRN: tower translated to MV `_t = (30, 30, 16)`.
        dot_vox::SceneNode::Transform {
            attributes: dict_empty(),
            frames: vec![dot_vox::Frame::new(dict_with("_t", "30 30 16"))],
            child: 5,
            layer_id: 0,
        },
        // 5 — nSHP referencing the tower.
        dot_vox::SceneNode::Shape {
            attributes: dict_empty(),
            models: vec![dot_vox::ShapeModel {
                model_id: 1,
                attributes: dict_empty(),
            }],
        },
    ];

    dot_vox::DotVoxData {
        version: 200,
        index_map: dot_vox::DEFAULT_INDEX_MAP.to_vec(),
        models,
        palette: dot_vox::DEFAULT_PALETTE.clone(),
        materials,
        scenes,
        layers: Vec::new(),
    }
}

/// Empty `dot_vox::Dict` (used as the no-attribute default for scene-graph
/// nodes that don't carry any attribute bytes).
fn dict_empty() -> dot_vox::Dict {
    StdHashMap::new().into_iter().collect()
}

/// `dot_vox::Dict` with a single `(k, v)` pair.
fn dict_with(k: &str, v: &str) -> dot_vox::Dict {
    let mut d: dot_vox::Dict = StdHashMap::new().into_iter().collect();
    d.insert(k.into(), v.into());
    d
}

/// Serialise the synthesised `DotVoxData` into a deterministic temp path
/// and return that path. Reusing a deterministic filename (rather than a
/// randomised tempfile) sidesteps the need for a `tempfile`-style dep and
/// is harmless: the file is overwritten every run.
pub fn write_vox_e2e_fixture_to_temp() -> std::io::Result<PathBuf> {
    let data = build_vox_e2e_fixture();
    let mut bytes = Vec::new();
    data.write_vox(&mut Cursor::new(&mut bytes))
        .map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("dot_vox write_vox failed: {e}"),
            )
        })?;
    let path = vox_e2e_fixture_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &bytes)?;
    Ok(path)
}

/// The on-disk path the synthesised `.vox` fixture is written to.
/// Deterministic — under the workspace `target/` (gitignored, persistent
/// across runs); the fixture content is regenerated every `--vox-e2e` run
/// so a stale file is safe.
pub fn vox_e2e_fixture_path() -> PathBuf {
    PathBuf::from(crate::e2e::E2E_SCREENSHOT_DIR).join("vox_e2e_fixture.vox")
}

/// Boot the e2e harness with the synthesised `.vox` fixture loaded
/// through the production `--vox <path>` ingestion path.
///
/// Returns the harness's `AppExit` — `AppExit::Success` if the regular
/// pipeline-error scan + node-dispatch + degenerate-frame floor checks
/// pass and the [`assert_vox_geometry_visible`] gate (substituted in for
/// the default-scene `assert_batch_6` region gate) passes.
///
/// Caller (`bin/e2e_render.rs` `--vox-e2e` branch) is responsible for
/// folding this into the binary exit code.
pub fn run_vox_e2e() -> AppExit {
    // 1) Serialise the fixture to a temp file so the production `--vox
    //    <path>` ingestion path runs verbatim (load_vox → std::fs::read →
    //    parse_vox_bytes → flatten_scene → build_world_from_vox).
    let path = match write_vox_e2e_fixture_to_temp() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "e2e_render --vox-e2e: failed to write synthesised .vox \
                 fixture: {e}"
            );
            return AppExit::error();
        }
    };
    println!(
        "e2e_render --vox-e2e: synthesised .vox fixture written to {} \
         ({} models, 2 nTRN translations)",
        path.display(),
        2
    );

    // 2) Set up `AppArgs` so:
    //     - `setup_test_grid` reads `GridPreset::Vox { path }` and calls
    //       `vox_import::load_vox(&path)` (`voxel/grid.rs:75-83`).
    //     - The driver's `ASSERT` step swaps the default-scene batch gate
    //       for `assert_vox_geometry_visible` (driver branch on
    //       `args.vox_e2e_mode`).
    let mut app_args = crate::AppArgs::default();
    app_args.grid_preset = crate::GridPreset::Vox { path, tiles: 1 };
    app_args.vox_e2e_mode = true;

    // 3) Run the harness the same way `--entities` does (Phase-C wave-3 —
    //    surface `AppArgs` overrides to the e2e binary). The standard
    //    `AppConfig::e2e()` window (256×256 non-resizable) is reused so
    //    the gate rect fractions stay calibrated.
    crate::run_e2e_render_with_args(app_args)
}

/// The vox-e2e region gate: assert the central screen rectangle (where
/// the synthesised emissive geometry projects to at the fixed e2e camera
/// pose) is meaningfully brighter than the no-geometry sky luminance
/// band.
///
/// **Why a central rect + a luminance floor (and not the default-scene
/// gates):**
/// The default-scene gates (`emissive_rect`, `solid_block_rect`,
/// `sky_rect` in `gates.rs:228-245`) sample fractional rectangles
/// calibrated against the *specific* voxel positions in
/// `voxel/grid.rs::build_default_volume`. With a synthesised `.vox` the
/// scene content is different — those rects would land on whatever
/// happens to be at those screen coords in the synthesised scene, with no
/// guarantee of what that is.
///
/// Instead, the vox-e2e fixture is sized + positioned so its emissive
/// cubes fill the central 40% × 40% of the framebuffer (the rect
/// fractions in [`VOX_GEOMETRY_RECT_FRACS`]). The fixture's MATL chunk
/// makes palette slot 1 emissive (`color_layered.x = 5.0` per the C#
/// `emit * (1+flux)² * 5` formula in
/// `voxel/vox_import.rs:790-791`), so the cubes render bright pre-GI —
/// the same way the default-scene `TY_EMISSIVE` block does. The
/// region-mean luminance lands well above the atmosphere-tinted sky
/// band (`~146` per the default-scene `sky_rect` baseline cited in
/// `03a-impl-vox-loading.md` Verification).
///
/// The threshold [`SKY_LUMINANCE_CEILING`] (160.0) sits just above the
/// measured sky band so a "no geometry loaded, only sky in frame"
/// failure trips the gate cleanly. The lit emissive region measures
/// ~240+ in calibration runs, so the gate has comfortable headroom and
/// does not flap.
pub fn assert_vox_geometry_visible(fb: &Framebuffer) -> Result<(), String> {
    let (fx0, fy0, fx1, fy1) = VOX_GEOMETRY_RECT_FRACS;
    let region = Rect::from_fractional(fb, fx0, fy0, fx1, fy1);
    let mean = fb.region_mean(region);
    let lum = Framebuffer::luminance(mean);

    println!(
        "e2e_render --vox-e2e: vox_geometry region luminance — \
         centre rect (fractional {fx0:.2}..{fx1:.2} × {fy0:.2}..{fy1:.2}, \
         pixel rect {}x{}..{}x{}) mean rgba {mean:?}, luminance {lum:.1} \
         (threshold > {SKY_LUMINANCE_CEILING:.0} — sky band ceiling)",
        region.x0, region.y0, region.x1, region.y1
    );

    if lum <= SKY_LUMINANCE_CEILING {
        return Err(format!(
            "vox-e2e gate FAIL — central screen region mean luminance \
             {lum:.1} is at or below the sky-band ceiling \
             ({SKY_LUMINANCE_CEILING:.0}). Either the synthesised .vox \
             fixture failed to render (the `--vox <path>` load path \
             silently produced an empty world, or the scene-graph \
             composition fix in `voxel/vox_import.rs::flatten_scene` \
             regressed — both cubes collapsed to origin), or the \
             render path is delivering only the atmosphere-tinted sky. \
             Mean rgba = {mean:?}; region pixel rect = ({}, {}, {}, {}). \
             Inspect target/e2e-screenshots/e2e_latest.png for the \
             captured frame.",
            region.x0, region.y0, region.x1, region.y1
        ));
    }
    Ok(())
}

/// Save the framebuffer to a vox-e2e-specific PNG alongside the standard
/// `e2e_latest.png` slot. Failure here is best-effort — the gate verdict
/// is unchanged either way.
pub fn save_vox_e2e_screenshot(fb: &Framebuffer) {
    let path =
        Path::new(crate::e2e::E2E_SCREENSHOT_DIR).join("vox_e2e_latest.png");
    match fb.save_png(&path) {
        Ok(()) => println!(
            "e2e_render --vox-e2e: screenshot saved to {}",
            path.display()
        ),
        Err(e) => eprintln!(
            "e2e_render --vox-e2e: vox_e2e_latest.png save failed: {e}"
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aadf::cell::{BlockCell, ChunkCell, VoxelCell};
    use crate::aadf::construct::ConstructedWorld;
    use crate::voxel::{vox_import, MaterialBase, VoxelTypeId};

    /// Test helper — decode the voxel at NAADF position `[x, y, z]` from a
    /// [`ConstructedWorld`]. Walks `ChunkCell::decode → BlockCell::decode →
    /// VoxelCell::decode`. Mirrors the helper in `vox_import::tests`. Used
    /// by the migrated v1-style position assertions on the sparse path.
    fn decoded_voxel_at(world: &ConstructedWorld, p: [u32; 3]) -> VoxelTypeId {
        let cx = p[0] / 16;
        let cy = p[1] / 16;
        let cz = p[2] / 16;
        let lcx = p[0] % 16;
        let lcy = p[1] % 16;
        let lcz = p[2] % 16;
        let bx = lcx / 4;
        let by = lcy / 4;
        let bz = lcz / 4;
        let lbx = lcx % 4;
        let lby = lcy % 4;
        let lbz = lcz % 4;
        let s = world.size_in_chunks;
        let ci = (cx + cy * s[0] + cz * s[0] * s[1]) as usize;
        if ci >= world.chunks.len() {
            return VoxelTypeId::EMPTY;
        }
        match ChunkCell::decode(world.chunks[ci]) {
            ChunkCell::Empty(_) => VoxelTypeId::EMPTY,
            ChunkCell::UniformFull(ty) => ty,
            ChunkCell::Mixed(bp) => {
                let block_idx = (bx + by * 4 + bz * 16) as usize;
                let block_word = world.blocks[bp.0 as usize + block_idx];
                match BlockCell::decode(block_word) {
                    BlockCell::Empty(_) => VoxelTypeId::EMPTY,
                    BlockCell::UniformFull(ty) => ty,
                    BlockCell::Mixed(vp) => {
                        let voxel_idx = (lbx + lby * 4 + lbz * 16) as usize;
                        let pair = voxel_idx / 2;
                        let lo = (world.voxels[vp.0 as usize + pair] & 0xFFFF) as u16;
                        let hi = ((world.voxels[vp.0 as usize + pair] >> 16) & 0xFFFF) as u16;
                        let half = if voxel_idx % 2 == 0 { lo } else { hi };
                        match VoxelCell::decode(half) {
                            VoxelCell::Empty(_) => VoxelTypeId::EMPTY,
                            VoxelCell::Full(ty) => ty,
                        }
                    }
                }
            }
        }
    }

    /// Count non-empty voxels by walking every world coord. The fixture is
    /// 64×32×64 = 131072 voxels so the walk is cheap.
    fn count_nonempty(world: &ConstructedWorld) -> u32 {
        let s = world.size_in_chunks;
        let sx = s[0] * 16;
        let sy = s[1] * 16;
        let sz = s[2] * 16;
        let mut n = 0u32;
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    if decoded_voxel_at(world, [x, y, z]) != VoxelTypeId::EMPTY {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// The synthesised fixture must round-trip cleanly through the
    /// production parser (`parse_vox_bytes`) and produce a non-empty
    /// world whose two emissive cubes land at distinct world positions.
    /// If both cubes stack at origin, the 03a-followup scene-graph
    /// composition has regressed (the production `--vox` load would
    /// then place the camera inside opaque material — the empty-scene
    /// symptom).
    #[test]
    fn fixture_round_trips_and_composes_two_distinct_models() {
        let data = build_vox_e2e_fixture();

        let mut bytes = Vec::new();
        data.write_vox(&mut Cursor::new(&mut bytes))
            .expect("write_vox failed");

        let imp =
            vox_import::parse_vox_bytes(&bytes).expect("parse_vox_bytes failed");

        // Combined MV bounds: x = (0..59), y = (0..59), z = (0..29) →
        // world MV size (60, 60, 30). Z↔Y swap → NAADF (60, 30, 60) →
        // chunks (4, 2, 4) → 64 × 32 × 64 voxels.
        assert_eq!(
            imp.world.size_in_chunks,
            [4, 2, 4],
            "vox-e2e fixture composed world AABB regressed — expected \
             a (4, 2, 4)-chunks world (60×30×60 MV → 64×32×64 NAADF)"
        );

        // The fixture has a 60×60×4 slab (14400 voxels) + a 20×20×28
        // tower (11200 voxels). Slab spans NAADF (0..59, 0..3, 0..59),
        // tower spans NAADF (20..39, 2..29, 20..39). Overlap is 20×2×20
        // = 800 voxels (`last-write-wins` collapses to a single type, but
        // they're the same type i=1 so the count is just slab + tower −
        // overlap).
        let nonempty = count_nonempty(&imp.world);
        let expected: u32 = 14400 + 11200 - 800;
        assert_eq!(
            nonempty, expected,
            "vox-e2e fixture expected exactly {} non-empty voxels (slab \
             + tower − overlap); got {} — scene-graph composition may \
             have regressed and stacked both models",
            expected, nonempty
        );

        // Confirm the central tower lands where the e2e camera looks:
        // NAADF look target is `(32, 16, 32)`. The tower interior covers it.
        assert_ne!(
            decoded_voxel_at(&imp.world, [32, 16, 32]),
            VoxelTypeId::EMPTY,
            "vox-e2e fixture tower must contain the e2e camera look target \
             NAADF (32, 16, 32) — otherwise the gate's central rect samples \
             empty space"
        );
        // And confirm the slab fills the ground (NAADF y=0..3).
        assert_ne!(
            decoded_voxel_at(&imp.world, [5, 0, 5]),
            VoxelTypeId::EMPTY,
            "vox-e2e fixture slab must fill the ground plane at a \
             corner location well away from the tower"
        );

        // Palette slot 1 → VoxelTypeId(2) after the `+1` shift, and
        // must be `MaterialBase::Emissive` because the fixture's MATL
        // chunk sets `_emit = 1.0` on slot 1.
        assert_eq!(
            imp.palette[2].material_base,
            MaterialBase::Emissive,
            "vox-e2e fixture's emissive palette mapping regressed — \
             palette slot 1 should land as MaterialBase::Emissive"
        );
        assert!(
            (imp.palette[2].color_layered.x - 5.0).abs() < 1e-4,
            "vox-e2e fixture emissive intensity should be 5.0 \
             (emit=1, flux=0 → 1*1*5); got {}",
            imp.palette[2].color_layered.x
        );
    }

    /// The composed world AABB must fit within `MAX_CHUNKS_PER_AXIS`. v2
    /// raised this cap from `32 → 1024`; the fixture is `(4, 2, 4)` chunks
    /// — well within both old and new caps.
    #[test]
    fn fixture_world_size_fits_within_gpu_producer_cap() {
        let data = build_vox_e2e_fixture();
        let mut bytes = Vec::new();
        data.write_vox(&mut Cursor::new(&mut bytes))
            .expect("write_vox failed");
        let imp =
            vox_import::parse_vox_bytes(&bytes).expect("parse_vox_bytes failed");
        for axis in 0..3 {
            assert!(
                imp.world.size_in_chunks[axis] <= vox_import::MAX_CHUNKS_PER_AXIS,
                "vox-e2e fixture axis {axis} ({} chunks) exceeds \
                 MAX_CHUNKS_PER_AXIS ({}); the fixture would fall back \
                 to the default test grid and the gate would not exercise \
                 the .vox path",
                imp.world.size_in_chunks[axis],
                vox_import::MAX_CHUNKS_PER_AXIS
            );
        }
    }

    /// The on-disk path must always sit inside the
    /// `target/e2e-screenshots/` directory — never an absolute path that
    /// might collide with user-authored fixtures elsewhere.
    #[test]
    fn fixture_path_is_under_target_dir() {
        let p = vox_e2e_fixture_path();
        assert!(
            p.starts_with(crate::e2e::E2E_SCREENSHOT_DIR),
            "vox-e2e fixture path {} must be under {}",
            p.display(),
            crate::e2e::E2E_SCREENSHOT_DIR
        );
        assert_eq!(
            p.file_name().and_then(|n| n.to_str()),
            Some("vox_e2e_fixture.vox"),
            "vox-e2e fixture filename regressed"
        );
    }
}
