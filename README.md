# bevy-naadf

A Bevy **0.19** port of [NAADF](https://github.com/cg-tuwien/NAADF) — a C#/MonoGame voxel
raytracing engine ("Nested Axis-Aligned Distance Fields", Ulschmid et al., CGF 2026) — to
Rust/Bevy, building natively on Linux.

The port is essentially complete — the NAADF substrate (three-layer chunk/block/voxel
cell hierarchy + AADF empty-space distance fields, CPU + GPU construction), the
DDA-with-AADF traversal, the full GI pipeline (atmosphere precompute, ReSTIR GI, sparse
bilateral denoiser, long-term TAA), and real-time GPU editing all land in the current
build.

## What it does

Builds a hard-coded voxel test grid (a ground slab, axis-aligned boxes, a sphere, one
emissive box) into the NAADF three-layer cell structure with CPU-computed AADFs, then
renders it with a two-pass custom render graph: a compute pass that casts a primary ray
per pixel through the AADF DDA traversal (`shootRay`, ported faithfully from NAADF's HLSL)
and writes a compact G-buffer + a flat sun-and-ambient shaded colour, then a fullscreen
blit that tonemaps it to the screen. Fly through it with the free camera.

## Requirements

- **Rust** stable (developed with 1.93).
- A GPU + driver wgpu can drive (Vulkan / Metal / DX12 native; WebGPU in the browser).

## Build & run

```sh
cargo run -p bevy-naadf --release   # or: just run
```

The first build compiles all of Bevy, so it takes a while.

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
`--no-default-features --features webgpu`, serves it with [Trunk](https://trunkrs.dev/),
and opens it in Chrome. `just web-build-release`
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

The on-screen overlay shows FPS, the active renderer, and per-pass NAADF render-node
GPU timings (`first-hit`, `final-blit`).

## Known caveats

- **Solari is experimental.** Some artifacts are expected — e.g. shimmer at world-cache LOD
  transitions, imperfect denoising on curved mirrors.

## Project layout

This is a Cargo workspace. `crates/bevy_naadf` is the renderer; `crates/voxel_noise` is a
[FastNoise2](https://github.com/Auburn/FastNoise2) wrapper carried over from
`bevy_voxel_world` — a native Rust API plus a C-ABI surface that builds to
`wasm32-unknown-emscripten` for an in-browser JS bridge. The noise crate is **not yet
wired into the renderer** — it is staged here so the 3D noise generation can be added
later without another restructure.

| Path                              | Responsibility                                                          |
| --------------------------------- | ----------------------------------------------------------------------- |
| `crates/bevy_naadf/src/main.rs`   | App wiring: plugins, CLI args, system scheduling                         |
| `crates/bevy_naadf/src/camera/`   | Free-fly camera spawn + the int+frac `PositionSplit` camera-relative type |
| `crates/bevy_naadf/src/voxel/`    | Voxel-type / material system + the hard-coded test-grid builder           |
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
| `.cargo/config.toml`              | `mold` linker config — no machine-specific paths                        |

## Roadmap

The NAADF port is essentially complete — the substrate, GI pipeline, and GPU-side
construction + editing all landed. The remaining work is content / streaming on top.

- [x] NAADF substrate + albedo first-hit — three-layer chunk/block/voxel cell hierarchy,
      CPU-side AADF construction + cuboid expansion, DDA-with-AADF traversal, int+frac
      `PositionSplit` camera, two-pass albedo first-hit WGSL render path
- [x] Long-term-memory TAA — 16-frame / 64-bit-sample temporal anti-aliasing pass,
      slotting between first-hit and the final blit
- [x] GI pipeline — atmosphere precompute, 4-plane-bounce first-hit, adaptive ~0.25-spp
      `rayQueueCalc` sampler, compressed ReSTIR GI with lit/unlit separation + 8×8
      screen-space regions + 12-iteration spatial pass, sparse bilateral denoiser,
      `base/` long-term TAA — 13 render-graph nodes; GI bounce lights the scene
- [x] GPU world construction & editing — GPU hashing construction per Algorithm 1,
      background chunk-AADF queue, flood-fill edit invalidation, real-time voxel edits
- [ ] Showcase a `.vox` world in the web build
- [ ] Procedural pseudo-infinite sliding world
- [ ] Persistent streamable world format

### Platform verification

- [x] Linux — native (Vulkan)
- [x] Linux — web (wasm32 / WebGPU via Trunk, Chrome)
- [ ] Windows — native
- [ ] Windows — web
- [ ] macOS — native (Metal)
- [ ] macOS — web
- [ ] iOS — web (Safari WebGPU)
- [ ] Android — web (Chrome WebGPU)
