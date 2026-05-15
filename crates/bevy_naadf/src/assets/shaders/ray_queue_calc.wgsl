// ray_queue_calc.wgsl — the adaptive ~0.25-spp ray-queue builder.
//
// Derives from: render/versions/base/rayQueueCalc.fx `calcRayQueue` +
// `calcRayQueueStore` (`09-design-b.md` §5.1, §5.6, §7). This is where NAADF's
// headline 2× GI speedup is realised: instead of casting a GI ray for every
// hit pixel every frame, it reads the TAA per-pixel accumulated sample-count
// signal (`taa_sample_accum.x & 0xFFFF`, exposed by Phase A-2) and only queues
// the pixels that actually need a ray this frame — well-converged pixels are
// rayed every 4th frame on a spatial-temporal pattern (~0.25 spp), freshly-
// disoccluded pixels every frame (1 spp).
//
// Two compute entry points:
//   * `calc_ray_queue`     — `[numthreads(64,1,1)]`: per pixel, run `should_ray`,
//     reserve a slot in the global counter via the inline group-shared
//     prefix-counter (`addToCounterAddressBuffer` ported inline — §5.6), write
//     the packed pixel position into the queue.
//   * `calc_ray_queue_store` — `[numthreads(1,1,1)]`: read the raw queued-pixel
//     count from `ray_queue_indirect[0]`, convert it to a workgroup count
//     `(v + 63) / 64`, write it back — that becomes the indirect dispatch arg
//     for `naadf_global_illum`.
//
// THE GROUP-SHARED PREFIX-COUNTER (`addToCounterAddressBuffer`,
// `commonOther.fxh:6-22`) is ported INLINE here, not as a shared `common.wgsl`
// function — it needs `var<workgroup>` declared at entry-point scope (a
// non-reusable cross-module construct — `09-design-b.md` §2.2 / §5.6). The HLSL
// `RWByteAddressBuffer groupCount` is `ray_queue_indirect`: the C# binds
// `rayQueueIndirectBuffer` into the shader's `groupCount` parameter
// (`WorldRenderBase.cs:280`), and the `.Load(0)` / `.Store(0)` /
// `.InterlockedAdd(0, ...)` byte-address ops at offset 0 are element `[0]` of
// the indirect buffer = `GroupCountX`. So the inline counter does
// `atomicAdd(&ray_queue_indirect[0], ...)`.
//
// HLSL `groupshared uint indexGroup = 0` initialises to zero at *module* scope;
// WGSL `var<workgroup>` is NOT auto-initialised, so `calc_ray_queue` zeroes
// `index_group` from `local_index == 0` before the first barrier (the C# relies
// on each dispatch starting with a freshly-zeroed `groupshared` — naga gives no
// such guarantee). Per `09-design-b.md` Batch-3 carry-forward: explicit `u32()`
// casts wherever the HLSL truncates implicitly.
//
// naga-oil import module — pulls in `GpuGiParams` + `GI_FLAG_SKIP_SAMPLES` from
// `gi_params.wgsl`.

#import "shaders/gi_params.wgsl"::{GpuGiParams, GI_FLAG_SKIP_SAMPLES}

// --- @group(0) — the ray-queue bindings -------------------------------------

@group(0) @binding(0) var<uniform> gi_params: GpuGiParams;
// The G-buffer (`firstHitData`) — read-only here; `.z & 0x7FFF` is the
// `voxelTypeRaw` (`0` ⇒ a primary-ray miss ⇒ no GI ray needed).
@group(0) @binding(1) var<storage, read> first_hit_data: array<vec4<u32>>;
// `pixelsToRender` — the ray queue: `pixel_count + 1` × `u32`, each entry a
// `pixelPos.x | (pixelPos.y << 16)` packed position (`rayQueueCalc.fx:33`).
@group(0) @binding(2) var<storage, read_write> ray_queue: array<u32>;
// `groupCount` — the HLSL `RWByteAddressBuffer`; element `[0]` is the queued-
// pixel counter (atomically incremented here) and then, after
// `calc_ray_queue_store`, the indirect `GroupCountX` for `naadf_global_illum`.
// Declared `atomic<u32>` so the inline prefix-counter's `atomicAdd` is legal
// (`09-design-b.md` §5.5 / §12 #5).
@group(0) @binding(3) var<storage, read_write> ray_queue_indirect: array<atomic<u32>, 5>;
// `taaSampleAccum` — the per-pixel TAA accumulated-colour+count buffer; the
// adaptive signal is `unpack2x16float(taa_sample_accum[id].x).x` (the f16 in
// the low 16 bits — the accepted-history-sample count). Read-only here.
@group(0) @binding(4) var<storage, read> taa_sample_accum: array<vec2<u32>>;

// --- the inline group-shared prefix-counter (addToCounterAddressBuffer) ------
// Group-shared scratch for the prefix-counter — `09-design-b.md` §5.6. HLSL
// `groupshared uint indexGroup = 0, indexGroupBase = 0` (`commonOther.fxh:4`).
var<workgroup> index_group: atomic<u32>;
var<workgroup> index_group_base: u32;

// `addToCounterAddressBuffer` ported inline (`commonOther.fxh:6-22`):
// each lane atomically reserves `add_count` slots in the group-local counter,
// lane 0 then atomically reserves the group's whole block in the global
// `ray_queue_indirect[0]` counter; the lane's final slot index is its
// group-local offset plus the group's global base. The three
// `workgroupBarrier()`s mirror the HLSL `GroupMemoryBarrierWithGroupSync()`s.
//
// Caller MUST have zeroed `index_group` (from `local_index == 0`) and issued a
// barrier before calling — see `calc_ray_queue`.
fn add_to_counter_address_buffer(local_index: u32, add_count: u32) -> u32 {
    workgroupBarrier();

    var index: u32 = 0u;
    if (add_count > 0u) {
        index = atomicAdd(&index_group, add_count);
    }

    workgroupBarrier();

    if (local_index == 0u) {
        // `buffer.InterlockedAdd(0, indexGroup, indexGroupBase)` — element [0]
        // of the indirect buffer is the global queued-pixel counter.
        index_group_base = atomicAdd(&ray_queue_indirect[0], atomicLoad(&index_group));
    }

    workgroupBarrier();

    return index + index_group_base;
}

// `shouldRay` (`rayQueueCalc.fx:12-21`) — the adaptive test. `accum` is the
// per-pixel accepted-history-sample count; a well-converged pixel (high accum)
// gets a large `mod_size` so it is rayed only every `mod_size`-th frame on a
// spatial-temporal pattern (`(frameIndex*4 + x + y) % mod_size == 0`); a fresh
// pixel (accum near 0) gets `mod_size == 1` ⇒ rayed every frame. When
// `skip_samples` is off, every hit pixel is rayed (1 spp).
fn should_ray(pos: vec2<u32>, accum: f32) -> bool {
    if ((gi_params.flags & GI_FLAG_SKIP_SAMPLES) == 0u) {
        return true;
    }
    let fac = accum / 2.0;
    // HLSL `round(clamp(fac * 2, 0, 3) + 1)` ⇒ mod_size ∈ {1,2,3,4}. The HLSL
    // `round` of a non-negative value truncates-after-+0.5; WGSL `round` is
    // round-to-nearest-even, which agrees for the .0 / .5-free values here.
    let mod_size = u32(round(clamp(fac * 2.0, 0.0, 3.0) + 1.0));
    // `(frameIndex * 4 + pos.x + pos.y) % modSize == 0`.
    return ((gi_params.frame_count * 4u + pos.x + pos.y) % mod_size) == 0u;
}

// `calcRayQueue` (`rayQueueCalc.fx:23-34`) — `[numthreads(64,1,1)]`.
@compute @workgroup_size(64, 1, 1)
fn calc_ray_queue(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let id = global_id.x;
    let pixel_count = gi_params.screen_width * gi_params.screen_height;

    // HLSL `groupshared` is zero-initialised at module scope; WGSL `var<workgroup>`
    // is not — zero `index_group` from lane 0, barrier so every lane sees it
    // before `add_to_counter_address_buffer`'s first `atomicAdd`.
    if (local_index == 0u) {
        atomicStore(&index_group, 0u);
    }
    workgroupBarrier();

    // The HLSL launches exactly `pixelThreadGroupCount` groups
    // (`= (w*h + 63) / 64`); the tail group's lanes past `pixel_count` must
    // still participate in the barriers — they just contribute `add_count = 0`.
    var pixel_pos = vec2<u32>(0u, 0u);
    var should_add = false;
    if (id < pixel_count) {
        pixel_pos = vec2<u32>(id % gi_params.screen_width, id / gi_params.screen_width);
        // `firstHitData[ID].z & 0x7FFF` is the voxelTypeRaw — `0` ⇒ primary-ray
        // miss ⇒ no GI ray. `accum` is the f16 in the low 16 bits of
        // `taaSampleAccum[ID].x` (`rayQueueCalc.fx:29`).
        let voxel_type_raw = first_hit_data[id].z & 0x7FFFu;
        let accum = unpack2x16float(taa_sample_accum[id].x).x;
        should_add = (voxel_type_raw != 0u) && should_ray(pixel_pos, accum);
    }

    // Reserve a queue slot via the inline group-shared prefix-counter — every
    // lane participates (the barriers require it); only `should_add` lanes
    // pass a non-zero `add_count`.
    let add_count = select(0u, 1u, should_add);
    let index = add_to_counter_address_buffer(local_index, add_count);

    if (should_add) {
        ray_queue[index] = pixel_pos.x | (pixel_pos.y << 16u);
    }
}

// `calcRayQueueStore` (`rayQueueCalc.fx:36-41`) — `[numthreads(1,1,1)]`.
// Converts the raw queued-pixel count in `ray_queue_indirect[0]` into the
// workgroup count `(v + 63) / 64` for the indirect `naadf_global_illum`
// dispatch. `GroupCountY` / `GroupCountZ` (`[1]` / `[2]`) were seeded to `1` on
// buffer creation and are untouched.
@compute @workgroup_size(1, 1, 1)
fn calc_ray_queue_store() {
    let group_count_value = atomicLoad(&ray_queue_indirect[0]);
    atomicStore(&ray_queue_indirect[0], (group_count_value + 63u) / 64u);
}
