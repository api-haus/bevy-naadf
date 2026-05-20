// sample_refine.wgsl — the 5-pass compressed-ReSTIR sample-refine stage.
//
// Derives from: render/versions/base/renderSampleRefine.fx (`09-design-b.md`
// §5.1, §5.7, §8.2). This is the `RefineBuckets` brightness-leveling stage: it
// takes the lit/unlit GI samples `renderGlobalIllum` wrote into the temporal
// rings, reprojects each into the current frame's 8×8 screen-space bucket grid
// via the camera-history rings, counts per-bucket lit/unlit totals, and
// brightness-levels the survivors with the `COLOR_DIF_PROB` exponential-
// difference probability table.
//
// FIVE compute entry points, dispatched as 5 separate render-graph nodes
// (`09-design-b.md` §4.7) — they interleave with `rayQueueCalc` / `globalIllum`
// in NAADF's dispatch order, so they cannot be one node:
//   * `clear_buckets_and_calc_mask`  — `[numthreads(64,1,1)]`: per-frame reset
//     of `ray_queue_indirect[0]` + `sample_counts[3+accumIndex]`, then per 8×8
//     bucket scan its 64 pixels' `first_hit_data` → normal-mask + min/max-dist.
//   * `compute_valid_history`        — `[numthreads(1,1,1)]`, 1 dispatch: walk
//     the 128-frame `sample_counts` ring back from `accum_index`, sum lit/unlit
//     counts until the ring-buffer capacity is hit ("up to 64 past frames"),
//     write `sample_counts[0..2]` + the two indirect-dispatch arg buffers.
//   * `count_valid_data_and_refine`  — `[numthreads(64,1,1)]`, indirect: for
//     each lit sample in the temporal ring, reproject into the 8×8 bucket grid
//     (using the INVERSE camera-history ring — §3.6), the `taa_dist_min_max`
//     validity test, `atomicAdd` the bucket's stored-count, write a
//     `refinedSample` into `valid_samples_refined`.
//   * `count_invalid_data`           — `[numthreads(64,1,1)]`, indirect: the
//     same reprojection for unlit samples, just `atomicAdd`s the bucket's
//     invalid count (no sample stored).
//   * `refine_buckets`               — `[numthreads(64,1,1)]`: per bucket the
//     `COLOR_DIF_PROB` brightness-leveling — compares each refined sample to
//     the bucket's max brightness, removes weakly-lit ones probabilistically,
//     compensates the survivors, writes ≤8 to `valid_samples_compressed`,
//     packs the bucket's lit/invalid ratio + count into `bucket_info`.
//
// PORT NOTES (`09-design-b.md` §5.7 + the Batch-3/4 carry-forwards):
// - `static uint compColorMaxStorage[32]` (HLSL function-scope `static`) → a
//   `var<function> comp_color_max_storage: array<u32, 32>` local — it is per-
//   thread scratch bounded by `effective_valid_count ≤ bucket_storage_count
//   = 32`.
// - `InterlockedAdd` on `globalIlumBucketInfo[i].x` → `bucket_info` is declared
//   `array<BucketInfoSlot>` where `BucketInfoSlot { x: atomic<u32>, y: u32 }`;
//   passes 1 / 5 access `.x` via `atomicStore` / `atomicLoad` (they do plain
//   reads/writes, but the module-wide binding type must be consistent).
// - `getRayDir(camRotOld[...], ...)` — `renderSampleRefine` binds the INVERSE
//   rotation-only camera-history ring (`taaSampleCamTransformInvers`) into its
//   `camRotOld` parameter (`WorldRenderBase.cs:346`, `09-design-b.md` §3.6), so
//   the port passes `camera_history[i].view_proj_inv` to the shared
//   `get_ray_dir` (which already takes an *inverse* view-proj).
// - HLSL implicit float→int truncation / scalar→vector broadcast: explicit
//   `u32()` / `i32()` casts and explicit `vec3` constructors throughout.
// - Every HLSL `mul(v, M)` is the column-vector `M * v` (the `05-review.md`
//   perspective-fix convention).
// - `#ifdef ENTITIES` blocks omitted — Phase B is entity-free (`09-design-b.md`
//   §1): `entityInstancesHistory` is not bound, `surfaceEntity` / `sampleEntity`
//   branches dropped.
// - CROSS-BATCH DEPENDENCY (`09-design-b.md` §11 Batch 4 step 13): until Batch 6
//   rewires the `base/` `ReprojectOld`, `taa_dist_min_max` is the zero-cleared
//   buffer, so `count_valid_data_and_refine` / `count_invalid_data` reject every
//   reprojected sample on the `distMinMax` test (`dist_min == dist_max == 0`).
//   The passes still dispatch clean — the data is just empty. Correct-but-empty,
//   never invalid.
//
// naga-oil import module — all 5 entry points share `@group(0)` =
// `sample_refine_bind_group`.

#import "shaders/gi_params.wgsl"::{GpuGiParams, GI_FLAG_IS_SAMPLE_LEVELING}
#import "shaders/render_pipeline_common.wgsl"::{
    FirstHitResult, SampleValid,
    get_hit_data_from_planes, get_ray_dir, get_specular_normals, get_tang,
}
#import "shaders/ray_tracing_common.wgsl"::{
    init_rand, next_rand, oct_encode, oct_decode, pdf_vndf_isotropic,
}
#import "shaders/color_compression.wgsl"::COLOR_DIF_PROB
#import "shaders/common.wgsl"::{find_coprime, next_pow2};

// Cap the indirect dispatch group count so we stay well within wgpu's
// `max_compute_workgroups_per_dimension` (WebGPU spec minimum / native
// wgpu default = 65535). When the indirect dispatch args exceed that
// limit, wgpu's indirect-validation compute pass overwrites the args
// with `(0,0,0)` (`wgpu-core/src/indirect_validation/dispatch.rs`), so
// `count_valid_data_and_refine` / `count_invalid_data` silently no-op —
// bucket counts stay empty, `valid_samples_compressed` stays empty,
// `spatial_resampling` finds no reservoirs, and the GI bounce light
// disappears. At 1920×1080 (pixel_count ≈ 2.07 M) the unclamped
// `next_pow2((pixel_count * 8 + 63) / 64) = 131 072` exceeds the limit;
// at 800×600 it stays well under, which is why the bug only manifests
// at higher resolutions / after a resize that grows the viewport
// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
// `## GI-bounce-on-resize fix (2026-05-16)`).
//
// 32768 = half the wgpu-default limit — leaves comfortable headroom,
// stays a power of two so the existing coprime shuffle stays correct,
// and on the worst case (`pixel_count * 8` invalid samples = 16.6 M at
// 1920×1080) still processes 32768 × 64 = 2.1 M samples per pass which
// distributes to ≈ 65 samples / 8×8 bucket — well above the 12-sample
// `< 12 ⇒ final_compressed_index = 0u` survival gate at line 706.
// This is a deliberate divergence from C# NAADF (`renderSampleRefine.fx
// :99-100`, `:117`, `:264`), which has the same latent overflow but
// never triggered it because the C# build was used at preset
// resolutions where `pixel_count * 8 / 64` stayed under 65 535
// (`docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md`
// `## GI-bounce-on-resize fix (2026-05-16) §Faithful-port deviation`).
const MAX_INDIRECT_GROUPS: u32 = 32768u;

// SSoT-4 — per-thread bucket-storage scratch capacity. The WGSL `array<T, N>`
// type-form requires a compile-time `N`, so we cannot read this from
// `gi_params.bucket_storage_count`. Instead the value is injected as a
// naga-oil shader-def by `NaadfPipelines::from_world` (mirrors the
// `TAA_SAMPLE_RING_DEPTH` pattern). The Rust SSoT is `gi::BUCKET_STORAGE_COUNT`.
const BUCKET_STORAGE_COUNT: u32 = #{BUCKET_STORAGE_COUNT}u;

fn capped_padded_groups(total: u32) -> u32 {
    return min(next_pow2((total + 63u) / 64u), MAX_INDIRECT_GROUPS);
}

// --- @group(0) — the shared sample-refine bindings --------------------------

@group(0) @binding(0) var<uniform> gi_params: GpuGiParams;
// `firstHitData` — the G-buffer, read-only.
@group(0) @binding(1) var<storage, read> first_hit_data: array<vec4<u32>>;
// `globalIlumBucketInfo` — the 8×8 screen-space region data. `.x` is atomically
// `InterlockedAdd`-ed by `count_valid_data_and_refine` / `count_invalid_data`,
// so the slot carries an `atomic<u32>` for `.x`; `.y` is plain.
@group(0) @binding(2) var<storage, read_write> bucket_info: array<BucketInfoSlot>;
// `globalIlumValidSamples` — the lit-sample ring, read-only here.
@group(0) @binding(3) var<storage, read> valid_samples: array<SampleValid>;
// `globalIlumValidSamplesRefined` — `bucket_count * 32` × `vec4<u32>`, written
// by `count_valid_data_and_refine`, read by `refine_buckets`.
@group(0) @binding(4) var<storage, read_write> valid_samples_refined: array<vec4<u32>>;
// `globalIlumValidSamplesCompressed` — `bucket_count * 8` × `vec4<u32>`, written
// by `refine_buckets` (the ≤8-survivor write).
@group(0) @binding(5) var<storage, read_write> valid_samples_compressed: array<vec4<u32>>;
// `globalIlumInvalidSamples` — the unlit-sample ring, read-only here.
@group(0) @binding(6) var<storage, read> invalid_samples: array<vec4<u32>>;
// `globalIlumSampleCounts` — the 128-frame accumulation ring (`128 + 3` slots).
// `sample_refine` does only plain loads/stores on it (no atomic adds — that is
// `globalIllum`'s job), so it is declared `array<vec2<u32>>` here, NOT the
// `atomic` `SampleCountSlot` form `naadf_global_illum.wgsl` uses for the SAME
// buffer (WGSL allows per-module binding-type views of one buffer).
@group(0) @binding(7) var<storage, read_write> sample_counts: array<vec2<u32>>;
// `taaDistMinMax` — `base/renderTaaSampleReverse.fx`'s `ReprojectOld` extra
// output (`09-design-b.md` §3.5). Read-only here; the per-pixel reprojection
// distance / specular-normal validity test reads it. CROSS-BATCH: zero-cleared
// until Batch 6 wires `ReprojectOld` to write it.
@group(0) @binding(8) var<storage, read> taa_dist_min_max: array<vec2<u32>>;
// `groupCount` — the C# binds `rayQueueIndirectBuffer` here
// (`WorldRenderBase.cs:270`); `clear_buckets_and_calc_mask` zeroes element `[0]`
// each frame (`renderSampleRefine.fx:39` — the per-frame queued-pixel-counter
// reset, §7.3). Plain `array<u32,5>` — `clearBucketsAndCalcMask` does a plain
// `.Store(0, 0)`, not an atomic op.
@group(0) @binding(9) var<storage, read_write> ray_queue_indirect: array<u32, 5>;
// `camRotOld` / `taaOldCamPosFromCurCamInt` — the 128-deep camera-history ring.
// `renderSampleRefine` binds the INVERSE rotation-only view-proj as `camRotOld`
// (`WorldRenderBase.cs:346` — `taaSampleCamTransformInvers`, NOT the non-inverse
// `taaSampleCamTransform` that `globalIllum` / `taaReverse` bind — §3.6); the
// port reads `view_proj_inv` for `get_ray_dir`.
@group(0) @binding(10) var<storage, read> camera_history: array<GpuCameraHistorySlot>;

// --- @group(1) — the indirect-dispatch arg buffers (compute_valid_history) --
// `globalIlumValidDispatch` / `globalIlumInvalidDispatch` are written ONLY by
// `compute_valid_history` (`renderSampleRefine.fx:99-100`), and then consumed
// as the INDIRECT dispatch source for `count_valid_data_and_refine` /
// `count_invalid_data`. wgpu forbids a buffer being bound `STORAGE_READ_WRITE`
// AND used as `INDIRECT` within one dispatch's usage scope — so they CANNOT sit
// in the shared `@group(0)` (the count passes would bind them rw while also
// indirect-dispatching off them). They live in their own `@group(1)`, bound
// ONLY by `naadf_sample_refine_valid_history_node`; the count passes get them
// purely as `dispatch_workgroups_indirect` sources (not a shader binding), so
// no usage conflict (`09-design-b.md` §3.7 — the design lists them in the
// sample-refine bind set; this split is the wgpu-faithful realisation of "the
// sample-refine passes need these buffers", forced by the indirect-vs-storage
// exclusivity rule — a deliberate port deviation from the "one shared bind
// group" wording, documented in `10-impl-b.md`).
@group(1) @binding(0) var<storage, read_write> valid_dispatch: array<u32, 5>;
@group(1) @binding(1) var<storage, read_write> invalid_dispatch: array<u32, 5>;

// One slot of the 8×8 bucket-grid region data (the C# `globalIlumBucketInfo`
// `Uint2`). `.x` is `InterlockedAdd`-ed by the count passes — `atomic<u32>`;
// `.y` is plain (the packed min/max-distance — written once by
// `clear_buckets_and_calc_mask`, never atomically).
struct BucketInfoSlot {
    x: atomic<u32>,
    y: u32,
}

// One slot of the 128-deep camera-history ring (mirrors
// `gpu_types::GpuCameraHistorySlot` / the `taa.wgsl` decl — 160 bytes).
struct GpuCameraHistorySlot {
    view_proj: mat4x4<f32>,
    view_proj_inv: mat4x4<f32>,
    cam_pos_from_cur_int: vec3<f32>,
    jitter: vec2<f32>,
}

// --- pass 1: clearBucketsAndCalcMask (renderSampleRefine.fx:33-69) ----------
//
// `[numthreads(64,1,1)]`, dispatched over `ceil(bucket_count / 64)` workgroups.
// Lane 0 of the whole dispatch does the per-frame ring-slot + queue-counter
// reset; every lane `< bucket_count` then scans its 8×8 bucket's 64 pixels'
// `first_hit_data` to build the bucket's normal-mask + min/max distance.
@compute @workgroup_size(64, 1, 1)
fn clear_buckets_and_calc_mask(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // The per-frame reset (`renderSampleRefine.fx:36-40`): clear this frame's
    // `sample_counts` ring slot + the `ray_queue_indirect[0]` queued-pixel
    // counter. NAADF does this here, before `rayQueueCalc` runs (§7.3) — Batch 4
    // takes this over from Batch 3's CPU re-seed in `prepare_gi`.
    if (global_id.x == 0u) {
        sample_counts[3u + gi_params.accum_index] = vec2<u32>(0u, 0u);
        ray_queue_indirect[0] = 0u;
    }

    if (global_id.x >= gi_params.bucket_count) {
        return;
    }

    var normal_mask: u32 = 0u;
    // `minTang` / `maxTang` are accumulated in the HLSL but never read (the
    // final `globalIlumBucketInfo` write uses only `normalMask` + min/max-dist);
    // kept for faithful provenance — naga discards the unused result.
    var min_tang: u32 = 16383u;
    var max_tang: u32 = 0u;
    let bucket_pos = vec2<u32>(
        global_id.x % gi_params.bucket_size_x,
        global_id.x / gi_params.bucket_size_x,
    );
    var min_dist: f32 = 9999999.0;
    var max_dist: f32 = 0.0;
    for (var y: u32 = 0u; y < 8u; y = y + 1u) {
        for (var x: u32 = 0u; x < 8u; x = x + 1u) {
            let pixel_pos = bucket_pos * 8u + vec2<u32>(x, y);
            if (pixel_pos.x >= gi_params.screen_width
                || pixel_pos.y >= gi_params.screen_height) {
                continue;
            }
            let first_hit = first_hit_data[pixel_pos.x + pixel_pos.y * gi_params.screen_width];
            let dist = unpack2x16float(first_hit.w & 0x7FFFu).x;
            min_dist = min(min_dist, dist);
            max_dist = max(max_dist, dist);

            let normal_tang_comp = get_tang(first_hit);
            let normal_comp = normal_tang_comp & 0x7u;
            min_tang = min(min_tang, normal_tang_comp >> 3u);
            max_tang = max(max_tang, normal_tang_comp >> 3u);
            normal_mask |= 1u << normal_comp;
        }
    }
    let dist_packed = pack2x16float(vec2<f32>(min_dist, 0.0)) & 0xFFFFu;
    let dist_packed_max = pack2x16float(vec2<f32>(max_dist, 0.0)) & 0xFFFFu;
    atomicStore(&bucket_info[global_id.x].x, (normal_mask >> 1u) & 0x3Fu);
    bucket_info[global_id.x].y = dist_packed | (dist_packed_max << 16u);
}

// --- pass 2: computeValidHistory (renderSampleRefine.fx:71-101) -------------
//
// `[numthreads(1,1,1)]`, a single dispatch. Walks the 128-frame `sample_counts`
// ring back from `accum_index`, summing lit/unlit counts until the ring-buffer
// capacity (`valid_sample_storage_count * w * h` / the unlit equivalent) is
// hit — that is the "up to 64 past frames" window (`02-research.md` §1.2.3).
// Then writes `sample_counts[0]` (the ring write cursors), `[1]` (the totals),
// `[2]` (the `findCoprime` shuffle seeds), and the two indirect-dispatch arg
// buffers.
@compute @workgroup_size(1, 1, 1)
fn compute_valid_history() {
    let max_size = vec2<u32>(
        gi_params.valid_sample_storage_count,
        gi_params.invalid_sample_storage_count,
    ) * gi_params.screen_width * gi_params.screen_height;
    var total_counts = vec2<u32>(0u, 0u);

    for (var i: u32 = 0u; i < gi_params.sample_max_accum; i = i + 1u) {
        let next_counts =
            sample_counts[3u + ((gi_params.accum_index + i) % gi_params.sample_max_accum)];
        total_counts += next_counts;
        if (total_counts.x > max_size.x || total_counts.y > max_size.y) {
            total_counts -= next_counts;
            break;
        }
    }

    var cur_sample_indices = sample_counts[0];
    let cur_sample_counts = sample_counts[3u + gi_params.accum_index];
    // The ring write cursor rolls back by this frame's count so a wrapping
    // ring read lands on the right window.
    cur_sample_indices.x =
        (cur_sample_indices.x + max_size.x - cur_sample_counts.x) % max_size.x;
    cur_sample_indices.y =
        (cur_sample_indices.y + max_size.y - cur_sample_counts.y) % max_size.y;
    sample_counts[0] = cur_sample_indices;
    sample_counts[1] = total_counts;

    // Cap the indirect dispatch group count at `MAX_INDIRECT_GROUPS` so
    // wgpu's indirect-validation pass does not zero the dispatch args on
    // high-resolution viewports — see the `MAX_INDIRECT_GROUPS` doc
    // above. The consumer passes (`count_valid_data_and_refine` /
    // `count_invalid_data`) apply the same cap when they recompute the
    // shuffle modulus, so the coprime walk stays a permutation of the
    // capped group range.
    let padded_valid_group_count = capped_padded_groups(total_counts.x);
    let padded_invalid_group_count = capped_padded_groups(total_counts.y);
    sample_counts[2] = vec2<u32>(
        find_coprime(padded_valid_group_count, gi_params.rand_counter),
        find_coprime(padded_invalid_group_count, gi_params.rand_counter_b),
    );
    valid_dispatch[0] = padded_valid_group_count;
    invalid_dispatch[0] = padded_invalid_group_count;
}

// `ShuffleGroup` (`renderSampleRefine.fx:103-106`) — the coprime-stride group
// shuffle so `count_valid_data_and_refine` / `count_invalid_data` walk the
// temporal ring in a decorrelated order.
fn shuffle_group(g_id: u32, num_groups: u32, group_shuffle_coprime: u32, offset: u32) -> u32 {
    return (group_shuffle_coprime * g_id + offset) % num_groups;
}

// The shared reprojection result the two count passes use — `count_valid_data_
// and_refine` needs the full set; `count_invalid_data` uses only `valid` +
// `bucket_index`.
struct ReprojResult {
    // `false` ⇒ the sample reprojected off-screen / failed a validity test ⇒
    // the caller `return`s.
    valid: bool,
    bucket_index: u32,
    // The reconstructed first-hit virtual path (only meaningful when `valid`).
    first_hit_result: FirstHitResult,
    // Camera-relative-old surface position (the HLSL `surfacePosVirtual`).
    surface_pos_virtual: vec3<f32>,
    // The old camera's int/frac position (D1).
    cam_pos_old_int: vec3<i32>,
    // The reprojected surface hit position in the *current* camera int frame.
    surface_hit_pos_new: vec3<f32>,
    // The old-frame ray direction (`getRayDir(camRotOld[...], ...)`).
    ray_dir_old: vec3<f32>,
    // The packed specular-normal codes of the old first-hit (for the sample
    // reconstruction in `count_valid_data_and_refine`).
    specular_normals_old: u32,
}

// The shared reprojection (`renderSampleRefine.fx:111-192` / `:258-335` — the
// two count passes run byte-identical reprojection logic). Reconstructs the
// old-frame virtual first-hit, reprojects its virtual surface position into the
// current camera, screen-space-bucket-indexes it, and runs the pdf-ratio + the
// `taa_dist_min_max` distance / specular-normal validity tests.
//
// `first_hit_packed` is the lit sample's `data_a` (`count_valid`) or the unlit
// sample's whole `vec4<u32>` (`count_invalid`) — both pack the first-hit code
// the same way (`renderGlobalIllum.fx` `compressSampleValid` / `Invalid`).
fn reproject_sample(first_hit_packed: vec4<u32>) -> ReprojResult {
    var r: ReprojResult;
    r.valid = false;

    let cam_pos_int = gi_params.cam_pos_int.xyz;
    let cam_pos_frac = gi_params.cam_pos_frac.xyz;

    let pixel_pos_old = vec2<u32>(
        first_hit_packed.y & 0x7FFFu,
        first_hit_packed.z & 0x7FFFu,
    );
    let frame_index_old = first_hit_packed.w & 0x7Fu;

    // `getRayDir(camRotOld[frameIndexOld], pixelPosOld, w, h)` — `camRotOld` is
    // the INVERSE rotation-only camera-history ring (§3.6); the shared
    // `get_ray_dir` takes an inverse view-proj, so this is a direct call.
    let ray_dir_old = get_ray_dir(
        camera_history[frame_index_old].view_proj_inv,
        pixel_pos_old,
        gi_params.screen_width,
        gi_params.screen_height,
        vec2<f32>(0.0, 0.0),
    );
    r.ray_dir_old = ray_dir_old;

    // The old camera's int/frac position relative to the current camera (D1).
    var cam_pos_old_frac = cam_pos_frac + camera_history[frame_index_old].cam_pos_from_cur_int;
    let cam_pos_old_int = cam_pos_int + vec3<i32>(floor(cam_pos_old_frac));
    cam_pos_old_frac = cam_pos_old_frac - floor(cam_pos_old_frac);
    r.cam_pos_old_int = cam_pos_old_int;

    let first_hit_result: FirstHitResult = get_hit_data_from_planes(
        first_hit_packed, cam_pos_old_int, cam_pos_old_frac, ray_dir_old,
    );
    r.first_hit_result = first_hit_result;

    let surface_roughness_comp = (first_hit_packed.w >> 7u) & 0xFFu;
    let surface_pos_virtual =
        camera_history[frame_index_old].cam_pos_from_cur_int
        + ray_dir_old * first_hit_result.dist;
    r.surface_pos_virtual = surface_pos_virtual;
    let specular_normals_old = get_specular_normals(first_hit_packed);
    r.specular_normals_old = specular_normals_old;

    // `surfaceHitPosNew = (camPosOldInt - camPosInt) + firstHitResult.pos`.
    r.surface_hit_pos_new =
        vec3<f32>(cam_pos_old_int - cam_pos_int) + first_hit_result.pos;

    // --- reproject (renderSampleRefine.fx:155-192) -------------------------
    // `materialState`: 1 ⇒ mirror (roughnessComp == 0), 0 ⇒ rough/diffuse.
    let material_state = select(0u, 1u, surface_roughness_comp == 0u);

    // `mul(float4(surfacePosVirtual, 1), camMatrix)` — the column-vector form.
    let screen_projection = gi_params.view_proj * vec4<f32>(surface_pos_virtual, 1.0);
    var ndc = screen_projection.xyz / screen_projection.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0
        || ndc.z < 0.0 || ndc.z > 1.0) {
        return r;
    }

    if (material_state == 0u) {
        let roughness = f32(surface_roughness_comp) / 255.0;
        let pdf_old = pdf_vndf_isotropic(
            reflect(ray_dir_old, first_hit_result.normal),
            -ray_dir_old,
            roughness,
            first_hit_result.normal,
        );
        let ray_dir_now = normalize(surface_pos_virtual);
        let pdf_now = pdf_vndf_isotropic(
            reflect(ray_dir_old, first_hit_result.normal),
            -ray_dir_now,
            roughness,
            first_hit_result.normal,
        );
        let fac = clamp(pdf_now / pdf_old, 0.0, 1.0);
        if (fac < 0.1) {
            return r;
        }
    }

    ndc.y = ndc.y * -1.0;
    let ndc01 = (ndc.xy + vec2<f32>(1.0, 1.0)) * 0.5;

    // HLSL `int2 screenPosBucket = ndc01 * float2(bucketSizeX, bucketSizeY)` —
    // implicit float→int truncation, explicit `i32()` here.
    let screen_pos_bucket = vec2<i32>(
        ndc01 * vec2<f32>(f32(gi_params.bucket_size_x), f32(gi_params.bucket_size_y)),
    );
    let screen_pos = vec2<i32>(
        ndc01 * vec2<f32>(f32(gi_params.screen_width), f32(gi_params.screen_height)),
    );
    let bucket_index =
        u32(screen_pos_bucket.x) + u32(screen_pos_bucket.y) * gi_params.bucket_size_x;
    let screen_index_with_type =
        u32(screen_pos.x) + u32(screen_pos.y) * gi_params.screen_width;
    r.bucket_index = bucket_index;

    // The `taa_dist_min_max` distance / specular-normal validity test
    // (`renderSampleRefine.fx:182-192`). CROSS-BATCH: `taa_dist_min_max` is the
    // zero-cleared buffer until Batch 6 — `dist_min_max == (0,0)` ⇒ `dist_cur`
    // fails the lower bound ⇒ every sample is rejected here. The pass still
    // dispatches clean; the data is just empty until Batch 6.
    let min_max = taa_dist_min_max[screen_index_with_type];
    let dist_min_max = vec2<f32>(
        unpack2x16float(min_max.x & 0xFFFFu).x,
        unpack2x16float(min_max.x >> 16u).x,
    );
    let dist_cur = length(surface_pos_virtual);

    let specular_normals_mask = vec3<u32>(
        1u << (specular_normals_old & 0x7u),
        1u << ((specular_normals_old >> 3u) & 0x7u),
        1u << ((specular_normals_old >> 6u) & 0x7u),
    );
    let valid_specular_normals = vec3<u32>(
        min_max.y & 0x7Fu,
        (min_max.y >> 7u) & 0x7Fu,
        (min_max.y >> 14u) & 0x7Fu,
    );
    if (dist_cur < dist_min_max.x * (1022.0 / 1024.0)
        || dist_cur > dist_min_max.y * (1026.0 / 1024.0)
        || any((specular_normals_mask & valid_specular_normals) == vec3<u32>(0u))) {
        return r;
    }

    r.valid = true;
    return r;
}

// --- pass 3: countValidDataAndRefine (renderSampleRefine.fx:108-253) --------
//
// `[numthreads(64,1,1)]`, dispatched INDIRECT off `valid_dispatch`. For each
// lit sample in the temporal ring (walked in the coprime-shuffled order):
// reproject it into the 8×8 bucket grid, `atomicAdd` the bucket's stored-count
// (the `1 << 6` field), and — if the bucket still has refined-slot space —
// reconstruct the secondary-bounce sample and write a `refinedSample` into
// `valid_samples_refined[bucket * 32 + slot]`.
@compute @workgroup_size(64, 1, 1)
fn count_valid_data_and_refine(
    // HLSL `SV_GroupID` → WGSL `@builtin(workgroup_id)`.
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let cam_pos_int = gi_params.cam_pos_int.xyz;
    let start_index = sample_counts[0].x;
    let total_count = sample_counts[1].x;
    let group_shuffle_coprime = sample_counts[2].x;
    let max_size = gi_params.valid_sample_storage_count
        * gi_params.screen_width * gi_params.screen_height;

    // Same cap as in `compute_valid_history` — see `MAX_INDIRECT_GROUPS`
    // doc. Coprime in `sample_counts[2].x` was found against the capped
    // count, so the modulus here must match exactly for the shuffle to
    // remain a permutation.
    let padded_group_count = capped_padded_groups(total_count);
    let shuffled_group = shuffle_group(
        workgroup_id.x, padded_group_count, group_shuffle_coprime, gi_params.rand_counter,
    );
    let id = shuffled_group * 64u + local_index;
    if (id >= total_count) {
        return;
    }

    let sample: SampleValid = valid_samples[(start_index + id) % max_size];
    let reproj = reproject_sample(sample.data_a);
    if (!reproj.valid) {
        return;
    }

    let first_hit_result = reproj.first_hit_result;
    let cam_pos_old_int = reproj.cam_pos_old_int;
    let surface_hit_pos_new = reproj.surface_hit_pos_new;

    // `materialState` (re-derived — `reproject_sample` does not return it).
    let surface_roughness_comp = (sample.data_a.w >> 7u) & 0xFFu;
    let material_state = select(0u, 1u, surface_roughness_comp == 0u);

    // `InterlockedAdd(globalIlumBucketInfo[bucketIndex].x, 1 << 6, oldBucketValue)`.
    let old_bucket_value = atomicAdd(&bucket_info[reproj.bucket_index].x, 1u << 6u);
    let old_bucket_valid = (old_bucket_value >> 6u) & 0xFFFu;

    if (old_bucket_valid < gi_params.bucket_storage_count) {
        // Decode the secondary-bounce sample direction from `data_b`.
        let sample_dir_comp = vec2<u32>(
            sample.data_b.w >> 10u,
            (sample.data_b.w & 0x3FFu) | ((sample.data_b.z & 0xFFFu) << 10u),
        );
        var sample_dir = oct_decode(vec2<f32>(sample_dir_comp) / 4194304.0); // pow(2, 22)
        var data2 = sample.data_b;
        data2.w = 0u;

        // Re-split the old first-hit virtual surface pos into int + frac (D1).
        var first_hit_pos_frac = first_hit_result.pos;
        let first_hit_pos_int = cam_pos_old_int + vec3<i32>(floor(first_hit_pos_frac));
        first_hit_pos_frac = first_hit_pos_frac - floor(first_hit_pos_frac);

        var sample_result: FirstHitResult = get_hit_data_from_planes(
            data2, first_hit_pos_int, first_hit_pos_frac, sample_dir,
        );
        let sample_pos_virtual =
            vec3<f32>(cam_pos_old_int - cam_pos_int)
            + first_hit_result.pos
            + sample_dir * sample_result.dist;

        if (sample_result.normal_tang == 0x1FFFFu) {
            sample_result.dist = 0.0;
        }

        // `#ifdef ENTITIES` block (`renderSampleRefine.fx:215-227`) omitted.

        if (sample_result.normal_tang != 0x1FFFFu) {
            let sample_dir_vec = sample_pos_virtual - surface_hit_pos_new;
            sample_result.dist = length(sample_dir_vec);
            sample_dir = normalize(sample_dir_vec);
        }

        // --- pack the refinedSample (renderSampleRefine.fx:236-249) --------
        var refined_sample: vec4<u32>;
        var surface_hit_pos_new_frac = surface_hit_pos_new;
        let surface_hit_pos_new_int = cam_pos_int + vec3<i32>(floor(surface_hit_pos_new_frac));
        surface_hit_pos_new_frac = surface_hit_pos_new_frac - floor(surface_hit_pos_new_frac);
        // HLSL `uint3 surfacePosInt = surfaceHitPosNewInt * 32 + (uint3)(frac * 32)`
        // — explicit `u32` truncation of the frac term.
        let surface_pos_int = vec3<u32>(surface_hit_pos_new_int * 32)
            + vec3<u32>(surface_hit_pos_new_frac * 32.0);
        let sample_normal_oct = oct_encode(sample_result.normal);
        // HLSL `uint2 sampleNormalComp = sampleNormalOct * 255.0` — implicit
        // float→uint truncation.
        let sample_normal_comp = vec2<u32>(sample_normal_oct * 255.0);
        let sample_dir_oct = oct_encode(sample_dir);
        let sample_dir_comp2 = vec2<u32>(sample_dir_oct * 2048.0);
        refined_sample.x = (sample.data_b.y & 0x7FFFu) | (surface_pos_int.y << 15u);
        refined_sample.y = sample_normal_comp.x
            | (sample_normal_comp.y << 8u)
            | ((pack2x16float(vec2<f32>(sample_result.dist, 0.0)) & 0xFFFFu) << 16u);
        // The two `<< 30` terms in the HLSL `.z` are an OR of the same bit
        // position — ported verbatim (the second is from `data_b.z >> 15`).
        refined_sample.z = sample_dir_comp2.x
            | (surface_pos_int.x << 11u)
            | (select(0u, 1u, (sample.data_b.y >> 15u) != 0u) << 30u)
            | (select(0u, 1u, (sample.data_b.z >> 15u) != 0u) << 30u)
            | (((sample.data_b.x >> 14u) & 0x1u) << 31u);
        refined_sample.w = sample_dir_comp2.y
            | (surface_pos_int.z << 11u)
            | (material_state << 30u);
        valid_samples_refined[
            reproj.bucket_index * gi_params.bucket_storage_count + old_bucket_valid
        ] = refined_sample;
    }
}

// --- pass 4: countInvalidData (renderSampleRefine.fx:255-338) ---------------
//
// `[numthreads(64,1,1)]`, dispatched INDIRECT off `invalid_dispatch`. The same
// reprojection as `count_valid_data_and_refine` for unlit samples, but it only
// `atomicAdd`s the bucket's invalid count (the `1 << 18` field) — no sample is
// reconstructed or stored.
@compute @workgroup_size(64, 1, 1)
fn count_invalid_data(
    // HLSL `SV_GroupID` → WGSL `@builtin(workgroup_id)`.
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let start_index = sample_counts[0].y;
    let total_count = sample_counts[1].y;
    let group_shuffle_coprime = sample_counts[2].y;
    let max_size = gi_params.invalid_sample_storage_count
        * gi_params.screen_width * gi_params.screen_height;

    // Same cap as in `compute_valid_history` — see `MAX_INDIRECT_GROUPS`
    // doc. Without it, at 1920×1080 the unclamped `next_pow2` here can
    // reach 131 072 and wgpu's indirect validation zeros the dispatch,
    // dropping every invalid-sample bucket count for the frame.
    let padded_group_count = capped_padded_groups(total_count);
    let shuffled_group = shuffle_group(
        workgroup_id.x, padded_group_count, group_shuffle_coprime, gi_params.rand_counter,
    );
    let id = shuffled_group * 64u + local_index;
    if (id >= total_count) {
        return;
    }

    let sample = invalid_samples[(start_index + id) % max_size];
    let reproj = reproject_sample(sample);
    if (!reproj.valid) {
        return;
    }

    // `InterlockedAdd(globalIlumBucketInfo[bucketIndex].x, 1 << 18)` — no
    // `oldValue` out-param needed (the unlit count is just incremented).
    atomicAdd(&bucket_info[reproj.bucket_index].x, 1u << 18u);
}

// --- pass 5: refineBuckets (renderSampleRefine.fx:340-417) ------------------
//
// `[numthreads(64,1,1)]`, dispatched over `ceil(bucket_count / 64)` workgroups.
// Per bucket: the `COLOR_DIF_PROB` brightness-leveling — find the bucket's max
// compressed-colour level, then for each of the ≤32 refined samples remove
// weakly-lit ones with `COLOR_DIF_PROB[maxColorDif]` probability, compensate the
// survivors (the `darkeningOffset` distance-variance term), write ≤8 to
// `valid_samples_compressed`, and pack the bucket's lit/invalid ratio + count
// into `bucket_info`.
@compute @workgroup_size(64, 1, 1)
fn refine_buckets(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let bucket_index = global_id.x;
    if (global_id.x >= gi_params.bucket_count) {
        return;
    }

    var rand = init_rand(vec3<u32>(global_id.x, gi_params.rand_counter, gi_params.rand_counter_b));

    let cur_bucket_x = atomicLoad(&bucket_info[bucket_index].x);
    let valid_count = (cur_bucket_x >> 6u) & 0xFFFu;
    // SSoT-4 — the `* 8u` IS `INVALID_SAMPLE_STORAGE_COUNT`. Read from the
    // uniform so the Rust constant (`gi::INVALID_SAMPLE_STORAGE_COUNT`) is the
    // single source of truth; the bit-packing at `:18u` shift stays unchanged.
    let invalid_count = (cur_bucket_x >> 18u) * gi_params.invalid_sample_storage_count;
    let original_lit_ratio = f32(valid_count) / f32(valid_count + invalid_count);
    // HLSL `int effectiveValidCount = min(validCount, bucketStorageCount)`.
    let effective_valid_count = min(valid_count, gi_params.bucket_storage_count);
    var effective_invalid_count =
        f32(invalid_count) * (f32(effective_valid_count) / (0.00000001 + f32(valid_count)));
    if (valid_count == 0u) {
        effective_invalid_count = f32(invalid_count);
    }

    var samples_comp_color_max: u32 = 0u;
    // The HLSL function-scope `static uint compColorMaxStorage[32]` — per-thread
    // scratch, bounded by `effective_valid_count ≤ bucket_storage_count`. The
    // capacity is the naga-oil shader-def `BUCKET_STORAGE_COUNT` (Rust SSoT
    // `gi::BUCKET_STORAGE_COUNT`).
    var comp_color_max_storage: array<u32, BUCKET_STORAGE_COUNT>;
    var distance_moments = vec2<f32>(0.0, 0.0);

    for (var i: u32 = 0u; i < effective_valid_count; i = i + 1u) {
        let cur_sample =
            valid_samples_refined[bucket_index * gi_params.bucket_storage_count + i];
        let comp_color = cur_sample.x & 0x7FFFu;
        let comp_color_max = max(
            comp_color & 0x1Fu,
            max((comp_color >> 5u) & 0x1Fu, (comp_color >> 10u) & 0x1Fu),
        );
        samples_comp_color_max = max(samples_comp_color_max, comp_color_max);
        comp_color_max_storage[i] = comp_color | (comp_color_max << 16u);
        let dist = unpack2x16float(cur_sample.y >> 16u).x;
        if (dist != 0.0) {
            distance_moments.x += dist;
            distance_moments.y += dist * dist;
        }
    }
    distance_moments /= f32(max(1u, effective_valid_count));
    let distance_mean = distance_moments.x;
    let distance_variance = distance_moments.y - distance_mean * distance_mean;

    let is_sample_leveling = (gi_params.flags & GI_FLAG_IS_SAMPLE_LEVELING) != 0u;

    var cur_compressed_index: u32 = 0u;
    var extra_invalid_samples: i32 = 0;
    for (var i: u32 = 0u; i < effective_valid_count; i = i + 1u) {
        let cur_storage = comp_color_max_storage[i];
        let comp_color = cur_storage & 0xFFFFu;
        let comp_color_max = cur_storage >> 16u;
        // `maxColorDif = isSampleLeveling ? samplesCompColorMax - compColorMax : 0`.
        let max_color_dif = select(
            0,
            i32(samples_comp_color_max) - i32(comp_color_max),
            is_sample_leveling,
        );
        let remove_prob = COLOR_DIF_PROB[max_color_dif];

        if (remove_prob > next_rand(&rand)) {
            extra_invalid_samples = extra_invalid_samples + 1;
        } else if (cur_compressed_index < 7u) {
            var cur_sample =
                valid_samples_refined[bucket_index * gi_params.bucket_storage_count + i];
            let dist = unpack2x16float(cur_sample.y >> 16u).x;
            let dist_fac = (dist - distance_mean) * (dist - distance_mean) / distance_variance;
            // HLSL `int darkeningOffset = dist == 0 ? 0 : max(0, pow(distFac,2)
            // * originalLitRatio * noiseSupressionFactor - 1)` — implicit
            // float→int truncation of the `max(0, ...)` term.
            var darkening_offset: i32 = 0;
            if (dist != 0.0) {
                darkening_offset = i32(max(
                    0.0,
                    dist_fac * dist_fac * original_lit_ratio
                        * gi_params.noise_suppression_factor - 1.0,
                ));
            }
            // `int3 newColorComp = max(0, int3(r,g,b) + maxColorDif - darkeningOffset)`.
            let new_color_comp = max(
                vec3<i32>(0),
                vec3<i32>(
                    i32(comp_color & 0x1Fu),
                    i32((comp_color >> 5u) & 0x1Fu),
                    i32((comp_color >> 10u) & 0x1Fu),
                ) + vec3<i32>(max_color_dif - darkening_offset),
            );

            cur_sample.x &= 0xFFFF8000u;
            let new_comp_color = u32(new_color_comp.x)
                | (u32(new_color_comp.y) << 5u)
                | (u32(new_color_comp.z) << 10u);
            cur_sample.x |= new_comp_color;
            valid_samples_compressed[
                bucket_index * gi_params.refined_bucket_storage_count + cur_compressed_index
            ] = cur_sample;
            cur_compressed_index = cur_compressed_index + 1u;
        }
    }

    let new_valid_count = i32(effective_valid_count) - extra_invalid_samples;
    let new_invalid_count = effective_invalid_count + f32(extra_invalid_samples);
    let valid_invalid_ratio =
        f32(new_valid_count) / f32(max(1, new_valid_count + i32(new_invalid_count)));
    var final_compressed_index = cur_compressed_index;
    if (f32(new_valid_count) + new_invalid_count < 12.0) {
        final_compressed_index = 0u;
    }

    let total_sample_count_comp = min(7u, (valid_count + invalid_count) / 64u);
    // The final `globalIlumBucketInfo[bucketIndex].x` repack
    // (`renderSampleRefine.fx:415`) — `(curBucket.x & 0x3F)` keeps the
    // normal-mask, the rest is the refined per-bucket header.
    atomicStore(
        &bucket_info[bucket_index].x,
        (cur_bucket_x & 0x3Fu)
            | (final_compressed_index << 6u)
            | ((pack2x16float(vec2<f32>(valid_invalid_ratio, 0.0)) & 0xFFFFu) << 9u)
            | (samples_comp_color_max << 24u)
            | (total_sample_count_comp << 29u),
    );
}
