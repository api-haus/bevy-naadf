// common.wgsl — shared constants + helpers.
//
// Derives from: render/common/common.fxh + commonConstants.fxh + settings.fxh
// (`03-design.md` §5.5). Phase-A subset.
//
// HLSL `common.fxh` is just an umbrella include + the `FLATTEN_INDEX` macro;
// `commonConstants.fxh` is `PI`; `settings.fxh` is the build flags + the
// `CHUNKTYPE` choice. Phase A is entity-free, so `CHUNKTYPE` is `u32`
// (`03-design.md` §7.5) — the chunk texture is `texture_3d<u32>` and that
// choice lives in `world_data.wgsl`, not here.
//
// WGSL has no `#include`; this is a naga-oil import module — other shaders
// pull symbols in via `#import "shaders/common.wgsl"::{...}`.

// Pi (HLSL `commonConstants.fxh` `PI`).
const PI: f32 = 3.141592653589793;

// Flatten a 3D position into a 1D index, x-fastest then y then z.
//
// HLSL `common.fxh`:
//   #define FLATTEN_INDEX(pos, sy, sz) mad(pos.z, sz, mad(pos.y, sy, pos.x))
//
// NAADF calls this with `(blockPosInChunk, 4, 16)` and
// `(voxelPosInBlock, 4, 16)` — note the *second* stride argument is the
// y-stride (4) and the *third* is the z-stride (16), i.e. for a 4×4×4 cell
// `flatten_index(p, 4u, 16u)`.
fn flatten_index(pos: vec3<u32>, stride_y: u32, stride_z: u32) -> u32 {
    return pos.z * stride_z + pos.y * stride_y + pos.x;
}
