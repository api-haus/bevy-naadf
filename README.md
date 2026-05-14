# bevy-naadf

A Bevy **0.19** port of [NAADF](https://github.com/cg-tuwien/NAADF) — a C#/MonoGame voxel
raytracing engine ("Nested Axis-Aligned Distance Fields", Ulschmid et al., CGF 2026) — to
Rust/Bevy, building natively on Linux.

The port is split into **four gated phases** (see the roadmap below). **Phase A** — the
NAADF substrate (the three-layer chunk/block/voxel cell hierarchy + AADF empty-space
distance fields), CPU-side AADF construction, the DDA-with-AADF traversal, and an
albedo-only first-hit WGSL render path — is complete. NVIDIA DLSS Ray Reconstruction
plumbing is kept dormant for the later GI phases.

## What it does

Builds a hard-coded voxel test grid (a ground slab, axis-aligned boxes, a sphere, one
emissive box) into the NAADF three-layer cell structure with CPU-computed AADFs, then
renders it with a two-pass custom render graph: a compute pass that casts a primary ray
per pixel through the AADF DDA traversal (`shootRay`, ported faithfully from NAADF's HLSL)
and writes a compact G-buffer + a flat sun-and-ambient shaded colour, then a fullscreen
blit that tonemaps it to the screen. Fly through it with the free camera.

No bounce lighting, no TAA, no GI yet — those are the later phases.

## Requirements

- **NVIDIA RTX GPU** with Vulkan ray tracing (RTX 20-series or newer). DLSS Ray
  Reconstruction is NVIDIA-only. Developed against an RTX 5080, driver 595.71.05.
- **Linux** with a recent NVIDIA driver and the Vulkan loader. DLSS + Solari are
  **Vulkan-only** — no DX12/Metal/GL fallback.
- **Rust** stable (developed with 1.93).
- System packages: `vulkan-headers`, `vulkan-icd-loader`, `clang`, `shaderc`
  (Arch/CachyOS package names; `clang` is needed by `dlss_wgpu`'s `bindgen` build step).

## One-time setup: the DLSS SDK

Bevy's `dlss` feature pulls in the [`dlss_wgpu`](https://github.com/bevyengine/dlss_wgpu)
crate, whose build script needs the NVIDIA DLSS SDK. Clone **v310.5.3** somewhere on your
machine — `git-lfs` must be installed so the `.so` binaries download (not just LFS pointers):

```sh
git clone --branch v310.5.3 --depth 1 https://github.com/NVIDIA/DLSS.git <path>
```

The SDK is covered by NVIDIA's own license (`LICENSE.txt` in that repo) and is **not**
vendored into this repository — clone it yourself.

Then point the build at it via environment variables. Machine-specific paths are kept out
of committed config — copy the template to a gitignored `.envrc` and fill in your paths:

```sh
cp .envrc.example .envrc
$EDITOR .envrc        # set DLSS_SDK and VULKAN_SDK
direnv allow          # direnv loads .envrc automatically; otherwise source it yourself
```

`DLSS_SDK` is read both at build time (by `dlss_wgpu`'s build script) and at runtime (NGX
uses it to locate the DLSS feature libraries), so make sure it is exported in whatever
shell you run `cargo` from.

## Build & run

```sh
cargo run --release
```

The first build compiles all of Bevy and `dlss_wgpu`, so it takes a while. A successful
build confirms the `dlss_wgpu` build script found `DLSS_SDK`, `VULKAN_SDK`, and `clang`.

## Controls

| Input                  | Action                                              |
| ---------------------- | --------------------------------------------------- |
| `W` `A` `S` `D`        | Move the camera                                     |
| `E` `Q`                | Fly up / down                                       |
| `Shift`                | Move faster                                         |
| Mouse                  | Look around                                         |
| `D`                    | Toggle DLSS Ray Reconstruction on/off (dormant in Phase A) |

The on-screen overlay shows FPS, the active renderer, DLSS-RR state, and per-pass NAADF
render-node GPU timings (`first-hit`, `final-blit`).

## Known caveats

- **Expect Vulkan validation errors.** They originate from a bug in the DLSS SDK itself
  (documented in the `dlss_wgpu` README) and are safe to ignore.
- **Solari is experimental.** Some artifacts are expected — e.g. shimmer at world-cache LOD
  transitions, imperfect denoising on curved mirrors.
- **`DLSS is not supported on this system`** in the log means NGX could not find the DLSS
  feature libraries — almost always because `DLSS_SDK` was not set in the environment the
  binary actually ran in. `DLSS_SDK` is read at *runtime*, not just at build time, so run
  via `cargo run` from a shell where `.envrc` is loaded (or otherwise export `DLSS_SDK`).
  `LD_LIBRARY_PATH` does not help here — NGX uses its own search path, seeded from `DLSS_SDK`.

## Project layout

| Path                    | Responsibility                                                          |
| ----------------------- | ----------------------------------------------------------------------- |
| `src/main.rs`           | App wiring: plugins, `DlssProjectId`, CLI args, system scheduling        |
| `src/camera/`           | Free-fly camera spawn + the int+frac `PositionSplit` camera-relative type |
| `src/voxel/`            | Voxel-type / material system + the hard-coded Phase-A test-grid builder  |
| `src/aadf/`             | The chunk/block/voxel cell encode/decode, CPU AADF construction + bounds |
| `src/world/`            | `WorldData` / `VoxelTypes` resources + the `GrowableBuffer` GPU wrapper  |
| `src/render/`           | Render-world extract/prepare, GPU types, pipelines, the render-graph nodes |
| `src/assets/shaders/`   | The WGSL render shaders (ported from NAADF's HLSL `Content/shaders/`)    |
| `src/hud.rs`            | Diagnostics overlay                                                     |
| `.envrc.example`        | Template for the gitignored `.envrc` (`DLSS_SDK`, `VULKAN_SDK`)          |
| `.cargo/config.toml`    | `mold` linker config — no machine-specific paths                        |

## Roadmap

The port is sequenced as four gated phases — each phase's design + implementation does not
begin until the prior phase is reviewed and confirmed runnable.

1. **Toolchain proof-of-concept** — Bevy 0.19 + Solari + DLSS-RR running on Linux. ✅
   *(superseded — Solari was stripped; it is reference-only, not the GI substrate.)*
2. **Phase A — NAADF substrate + albedo first-hit.** ✅ The three-layer chunk/block/voxel
   cell hierarchy, CPU-side AADF construction + cuboid expansion, the DDA-with-AADF
   traversal, the int+frac `PositionSplit` camera, and a two-pass albedo first-hit WGSL
   render path (compute first-hit → fullscreen blit). Flat-lit, no bounce lighting, no TAA.
3. **Phase A-2 — long-term-memory TAA.** ✅ The 16-frame / 64-bit-sample temporal
   anti-aliasing pass, slotting between first-hit and the final blit.
4. **Phase B — the GI pipeline.** ✅ The full NAADF `WorldRenderBase` real-time GI
   pipeline: the atmosphere precompute, the 4-plane-bounce first-hit, the adaptive
   ~0.25-spp `rayQueueCalc` sampler, compressed ReSTIR GI (lit/unlit separation, 8×8
   screen-space regions, the 12-iteration spatial pass), the sparse bilateral denoiser,
   and the `base/` long-term TAA (`ReprojectOld` + `CalcNewTaaSample`). 13 render-graph
   nodes; the GI bounce lights the scene.
5. **Phase C — GPU world construction & editing.** The GPU hashing construction
   (Algorithm 1), the background chunk-AADF queue, and flood-fill edit invalidation — a
   scalability / editability track, not a rendering foundation.
