# 27 — Embed Roboto Regular font, apply to dev UI

## 1. Font chosen

**Roboto Regular**, version from system packages (CachyOS / Arch `ttf-roboto`).  
Source: `/usr/share/fonts/TTF/Roboto-Regular.ttf` (local copy — no download needed).  
Copied to: `crates/bevy_naadf/src/assets/fonts/Roboto-Regular.ttf` (460,324 bytes).  
License: Apache 2.0 — see §2.

## 2. License file

`crates/bevy_naadf/src/assets/fonts/Roboto-LICENSE.txt` — full Apache 2.0 text with Google's copyright notice (`Copyright 2011 Google Inc.`).

## 3. Embedding mechanism

**`include_bytes!`** (not `embedded_asset!`).

Rationale: the project's asset server is pointed at `src/assets/` via `AssetPlugin.file_path`, and the existing shaders / textures load via that path at runtime. For the font we want a compile-time guarantee (no missing-file startup failure, single self-contained binary). `include_bytes!` gives that with zero new plugins or build-script machinery. `embedded_asset!` / `EmbeddedAssetPlugin` is the right choice when you want the asset to be hot-reloadable or addressable by path string; for a UI font that never changes at runtime, `include_bytes!` is simpler.

`Font::from_bytes(data, "Roboto")` converts the raw bytes into a `bevy_text::Font` at startup; the handle is stored as `FontSource::Handle(handle)` in the `DevFont` resource.

## 4. Files modified

| Path | Change |
|------|--------|
| `crates/bevy_naadf/src/assets/fonts/Roboto-Regular.ttf` | New — 460 KB font binary |
| `crates/bevy_naadf/src/assets/fonts/Roboto-LICENSE.txt` | New — Apache 2.0 license text |
| `crates/bevy_naadf/src/lib.rs` | Added `ROBOTO_REGULAR_BYTES` static, `DevFont` resource, `load_dev_font` startup system; wired into `build_app_with_args` before `setup_hud`/`setup_panel` |
| `crates/bevy_naadf/src/hud.rs` | `setup_hud` now takes `Res<DevFont>`; `TextFont.font` set to `dev_font.0.clone()` |
| `crates/bevy_naadf/src/panel.rs` | `setup_panel` now takes `Res<DevFont>`; all 4 `TextFont` spawn sites updated |

## 5. Asset binary size

```
460324 bytes  crates/bevy_naadf/src/assets/fonts/Roboto-Regular.ttf
```

## 6. Gate exit codes

1. `cargo build --workspace` — exit 0, no new warnings on touched files.
2. `cargo test -p bevy-naadf --lib` — **119 passed**, 1 ignored (baseline unchanged).
3. `cargo run --release --bin e2e_render` — exit 0; luminance emissive 247.0, solid 242.1, sky 145.9. PASS.
4. `cargo run --release --bin e2e_render -- --entities` — exit 0; luminance emissive 247.1, solid 242.0, sky 145.9. PASS.

## 7. Notes for the user

- Restart `cargo run` (or `cargo run --release`) to see Roboto rendered in the HUD and quality panel instead of FiraSans.
- The `[↑↓←→]` arrow glyphs in the legend line now render correctly — Roboto covers U+2190..U+2193.
- The 460 KB font is baked into the binary at compile time; no runtime asset file is required.
- To add a second embedded font: add a second `static` with `include_bytes!`, add a field to `DevFont`, load it in `load_dev_font`.
