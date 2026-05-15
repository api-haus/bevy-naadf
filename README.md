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
cargo run -p bevy-naadf --release   # or: just run
```

The first build compiles all of Bevy and `dlss_wgpu`, so it takes a while. A successful
build confirms the `dlss_wgpu` build script found `DLSS_SDK`, `VULKAN_SDK`, and `clang`.

### Texture-array assets

`crates/bevy_naadf/src/texture_array/` is a small Bevy asset pipeline that bakes
`*.texarray.ron` definitions — at once a **channel combiner** and an **array packer** —
into 2D-array textures bindable as `texture_2d_array` in WGSL (for the future terrain
raymarching path). A definition names a pixel format and a list of layer *elements*; each
element wires its four output channels to a source texture + channel, optionally inverted
(see `crates/bevy_naadf/src/assets/textures/sample.texarray.ron`).

The same definition drives two paths:

- **Loaded** (default; the only path on wasm) — `TextureArrayLoader` is a normal asset
  loader: `asset_server.load::<Image>("textures/foo.texarray.ron")` bakes it into an
  *uncompressed* RGBA8 2D-array `Image` on load. This is what the production app uses.
- **Processed** — `just bake-texarrays` (`cargo run --bin bake`) runs a headless
  `AssetMode::Processed` app whose `AssetProcessor` Basis-Universal-supercompresses each
  array into a `.basis` file under `imported_assets/`; Bevy's runtime transcoder then
  decodes it per-GPU at load. `AssetMode::Processed` is app-global, so it is confined to
  the dedicated `bake` binary — the production app and the e2e harness stay `Unprocessed`.

Basis is **native-only**: the `basis-universal` C++ encoder does not cross-compile to
`wasm32-unknown-unknown`, so the web build always takes the loaded (uncompressed) path.
Source textures referenced by a `*.texarray.ron` need a `Load`-action `.png.meta` sidecar
(the baker needs raw pixels) — see the `crates/bevy_naadf/src/texture_array/` module docs.

### Web build (WebGPU, wasm32)

`just web` builds the `bevy-naadf` binary for `wasm32-unknown-unknown` with
`--no-default-features --features webgpu` (the `dlss` feature has no web build), serves it
with [Trunk](https://trunkrs.dev/), and opens it in Chrome. `just web-build-release`
produces the optimised `crates/bevy_naadf/dist/` bundle without serving. The Playwright
smoke test under `e2e/` (`just install-e2e` then `just test-wasm-full`) loads that bundle
headless and asserts it boots without panics. `.github/workflows/deploy-cloudflare.yml`
builds the release bundle and deploys it to Cloudflare Pages, with the large wasm binary
served from R2 via the `workers/r2-proxy/` worker (which stamps the CORS / CORP headers
the cross-origin-isolated page needs).

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

This is a Cargo workspace. `crates/bevy_naadf` is the renderer; `crates/voxel_noise` is a
[FastNoise2](https://github.com/Auburn/FastNoise2) wrapper carried over from
`bevy_voxel_world` — a native Rust API plus a C-ABI surface that builds to
`wasm32-unknown-emscripten` for an in-browser JS bridge. The noise crate is **not yet
wired into the renderer** — it is staged here so the 3D noise generation can be added
later without another restructure.

| Path                              | Responsibility                                                          |
| --------------------------------- | ----------------------------------------------------------------------- |
| `crates/bevy_naadf/src/main.rs`   | App wiring: plugins, `DlssProjectId`, CLI args, system scheduling        |
| `crates/bevy_naadf/src/camera/`   | Free-fly camera spawn + the int+frac `PositionSplit` camera-relative type |
| `crates/bevy_naadf/src/voxel/`    | Voxel-type / material system + the hard-coded Phase-A test-grid builder  |
| `crates/bevy_naadf/src/aadf/`     | The chunk/block/voxel cell encode/decode, CPU AADF construction + bounds |
| `crates/bevy_naadf/src/world/`    | `WorldData` / `VoxelTypes` resources + the `GrowableBuffer` GPU wrapper  |
| `crates/bevy_naadf/src/render/`   | Render-world extract/prepare, GPU types, pipelines, the render-graph nodes |
| `crates/bevy_naadf/src/texture_array/` | `*.texarray.ron` channel-combiner / array-packer asset pipeline — loader + Basis `AssetProcessor` |
| `crates/bevy_naadf/src/bin/bake.rs` | Headless `AssetMode::Processed` runner — `just bake-texarrays` bakes `*.texarray.ron` → `.basis` arrays |
| `crates/bevy_naadf/src/assets/shaders/` | The WGSL render shaders (ported from NAADF's HLSL `Content/shaders/`) |
| `crates/bevy_naadf/src/hud.rs`    | Diagnostics overlay                                                     |
| `crates/bevy_naadf/index.html` / `Trunk.toml` | The Trunk WebGPU (wasm32) web-build entry point             |
| `crates/bevy_naadf/{_headers,sw.js,init.js.template}` | Cloudflare Pages headers + caching SW + the CI wasm loader |
| `crates/voxel_noise/`             | FastNoise2 wrapper — native API (`src/native.rs`, `src/presets.rs`) + the Emscripten C-ABI module (`Makefile`, `js/`) |
| `e2e/`                            | Playwright smoke test for the web build (`serve.mjs` + `tests/`)        |
| `workers/r2-proxy/`               | Cloudflare Worker — serves the R2-hosted wasm with CORS / CORP headers  |
| `scripts/`                        | `patch-wasm-loading.sh` (CI R2 loader injection) + `lint/wasm-compat.sh` |
| `.github/workflows/`              | `deploy-cloudflare.yml` — builds both crates + deploys to Pages         |
| `.envrc.example`                  | Template for the gitignored `.envrc` (`DLSS_SDK`, `VULKAN_SDK`)          |
| `.cargo/config.toml`              | `mold` linker config — no machine-specific paths                        |

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
