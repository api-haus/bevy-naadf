// noise_oracle_dispatch.wgsl — thin compute dispatcher that drives the
// `--wgsl-noise-oracle` gate (`docs/orchestrate/streaming-world/02b-design-plan-b.md` § C).
//
// Each invocation samples ONE `(state, x, y, z)` test case and writes the
// resulting noise value into `output[i]`. The CPU oracle in
// `streaming::noise_fastnoiselite_cpu_oracle` computes the same noise on the
// CPU; the gate compares the two arrays for bit-near-equality (`< 1e-5`).
//
// Bind groups:
//   - `sample_points` (storage, read): N test cases, each
//     `vec4<f32>` = (x, y, z, _pad).
//   - `states` (storage, read): N `FnlState` configurations, one per test case.
//   - `output` (storage, read_write): N `f32` noise values.
//
// Each `FnlState` is per-sample so we can exercise different noise/fractal/etc
// configurations in one dispatch. The total dispatch size is `N` invocations,
// distributed across workgroups of 64.

// Inline the noise module — we intentionally `include_str!` + concat in Rust
// rather than rely on Bevy's `#import` cross-module resolution. The Rust GPU
// runner pastes `noise_fastnoiselite.wgsl` ABOVE this file's `// @begin` marker.
// See `streaming::noise_fastnoiselite::build_oracle_dispatch_shader_src`.

// @begin

struct SamplePoint {
    pos: vec4<f32>,
};

@group(0) @binding(0) var<storage, read> sample_points: array<SamplePoint>;
@group(0) @binding(1) var<storage, read> states: array<FnlState>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
@group(0) @binding(3) var<uniform> dispatch_params: DispatchParams;

struct DispatchParams {
    // `count` = number of valid samples. WGSL dispatch always launches
    // multiples-of-64 threads; per-invocation bounds-check guards the tail.
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@compute @workgroup_size(64, 1, 1)
fn dispatch_oracle(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= dispatch_params.count) {
        return;
    }
    let p = sample_points[i].pos;
    let s = states[i];
    output[i] = fnl_get_noise_3d(s, p.x, p.y, p.z);
}
