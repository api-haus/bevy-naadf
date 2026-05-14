# bevy-naadf

A Bevy **0.19** application with **Solari** realtime raytraced lighting denoised by
**NVIDIA DLSS Ray Reconstruction**, building natively on Linux.

This is the foundation for porting [NAADF](https://github.com/cg-tuwien/NAADF) — a
C#/MonoGame voxel raytracing engine ("Nested Axis-Aligned Distance Fields", CGF 2026) —
to Rust/Bevy and extending it with DLSS Ray Reconstruction. This proof of concept renders
a generic Solari scene so the toolchain (DLSS SDK build, Vulkan ray tracing, the Bevy 0.19
API) is proven before the larger port begins.

## What it does

A self-contained procedural scene — an open box with coloured walls, a few blocks, a
near-mirror metallic sphere, and a bright emissive ceiling slab — lit entirely by Solari's
raytraced global illumination and reflections. Solari's raw output is noisy; DLSS Ray
Reconstruction denoises and upscales it. Press **D** to toggle DLSS-RR and see the
difference directly.

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

### Reference pathtracer

```sh
cargo run --release -- --pathtracer
```

Swaps Solari's realtime lighting for its reference pathtracer, which converges to a clean
ground-truth image over time (no DLSS-RR — it denoises itself by accumulating samples).
Useful for judging whether the realtime + DLSS-RR result is faithful.

## Controls

| Input                  | Action                                              |
| ---------------------- | --------------------------------------------------- |
| `W` `A` `S` `D`        | Move the camera                                     |
| `Shift`                | Move faster                                         |
| Mouse                  | Look around                                         |
| `D`                    | Toggle DLSS Ray Reconstruction on/off (realtime mode) |

The on-screen overlay shows FPS, the active renderer, DLSS-RR state, and per-pass Solari /
DLSS GPU timings.

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

| File                   | Responsibility                                                       |
| ---------------------- | -------------------------------------------------------------------- |
| `src/main.rs`          | App wiring: plugins, `DlssProjectId`, CLI args, system scheduling     |
| `src/scene.rs`         | Procedural scene — meshes, materials, lights, `RaytracingMesh3d`      |
| `src/camera.rs`        | Camera spawn (Solari + conditional DLSS-RR) and the runtime `D` toggle |
| `src/hud.rs`           | Diagnostics overlay                                                  |
| `.envrc.example`       | Template for the gitignored `.envrc` (`DLSS_SDK`, `VULKAN_SDK`)       |
| `.cargo/config.toml`   | `mold` linker config — no machine-specific paths                     |

## Roadmap

1. **(this milestone)** Bevy 0.19 + Solari + DLSS-RR running on Linux. ✅
2. Port NAADF's voxel data structures and AADF compute pipeline to Rust/Bevy.
3. Integrate DLSS Ray Reconstruction into the NAADF renderer as its denoiser.
