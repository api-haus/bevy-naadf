# 08-web-ci-audit — web build + CI deploy touch points for .cvox

## delegate-auditor findings (2026-05-19)

## TL;DR

The web deploy path today fetches `oasis_hard_cover.vox` at startup — a `.vox` binary
LFS-tracked in git and uploaded to the `bevy-naadf-assets` R2 bucket at key
`models/oasis_hard_cover.vox` by `.github/workflows/deploy-cloudflare.yml`. The
constant `DEFAULT_VOX_URL` in `web_vox.rs:45-46` hardcodes the full HTTPS URL to
that R2 key. The **runtime fetch path already accepts `.cvox` bytes** — because the
fetched bytes flow through `submit_pending_bytes` → `QUEUED_FOR_INSTALL` →
`spawn_wasm_vox_parse` → `crate::voxel::grid::parse_to_imported_vox` → which, post-
`05-impl`, is a one-line shim over `voxel_dispatch::parse_voxel_bytes` (magic-byte
dispatch). So the runtime side is ready; only the **asset shipping side** needs to
change. Specifically: (1) `oasis.cvox` already exists in-tree at
`crates/bevy_naadf/assets/test/oasis.cvox` (~6.8 MB, committed directly to git,
NOT LFS-tracked), (2) the `.gitattributes` has no `*.cvox` LFS rule, (3) CI has no
upload step for `.cvox`, (4) `DEFAULT_VOX_URL` points at the `.vox` key, and (5) the
Playwright `vox-loading.spec.ts` test fixture URL is hardcoded to
`/test-fixtures/oasis_hard_cover.vox` but the `serve.mjs` MIME map has no `.cvox`
entry. Switching the web default to `.cvox` requires touching 5 files: the CI
workflow (add upload step), `web_vox.rs` (`DEFAULT_VOX_URL`), `e2e/serve.mjs` (MIME
map), `e2e/tests/vox-loading.spec.ts` (fixture URL), and `.gitattributes` (add
`*.cvox` LFS rule if the file will be LFS-managed — it currently is not).

---

## A. Asset shipping path

### A.1 The shipped .vox file (today)

| Property | Value |
|---|---|
| File on disk | `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` |
| Size | ~84.9 MB (`84911723` bytes) |
| Git tracked? | Yes — via LFS (`git lfs ls-files` shows `4c9eb28a59 * crates/bevy_naadf/assets/test/oasis_hard_cover.vox`) |
| LFS attribute | `.gitattributes:3` — `*.vox filter=lfs diff=lfs merge=lfs -text` |
| Commits | Added in `929a5a2` (`fix(edit): drain pending_edits… + LFS Oasis fixture`); LFS enablement in CI added by `746b196` |

`oasis.cvox` also exists at `crates/bevy_naadf/assets/test/oasis.cvox`:

| Property | Value |
|---|---|
| File on disk | `crates/bevy_naadf/assets/test/oasis.cvox` |
| Size | ~6.8 MB (`6791493` bytes — ZIP-compressed binary) |
| Git tracked? | Yes — committed directly (NOT via LFS). `git ls-files --error-unmatch` returns the path. Commit `b087a27` (`checkpoint: cvox import + voxel dispatch + oasis instance-count impl`). |
| LFS attribute | **None** — `.gitattributes` covers `*.vox` but has no `*.cvox` rule. |

### A.2 CI workflow that uploads

**File:** `.github/workflows/deploy-cloudflare.yml`
**Only workflow file in `.github/workflows/`.**

#### LFS materialization
`.github/workflows/deploy-cloudflare.yml:93-98`:
```yaml
- uses: actions/checkout@v4
  with:
    # Required: `.vox` assets uploaded to R2 below are LFS-tracked.
    # Without this, the R2 upload ships the LFS pointer file …
    lfs: true
```
`lfs: true` was the fix introduced in commit `746b196`. This materialises
`.vox` files before upload.

#### Upload step (`.vox`)
`.github/workflows/deploy-cloudflare.yml:182-190`:
```yaml
- name: Upload default .vox to R2
  env:
    CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}
    CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
  run: |
    npx wrangler r2 object put \
      bevy-naadf-assets/models/oasis_hard_cover.vox \
      --file crates/bevy_naadf/assets/test/oasis_hard_cover.vox \
      --remote
```

- **R2 bucket:** `bevy-naadf-assets`
- **R2 key:** `models/oasis_hard_cover.vox`
- **Source file:** `crates/bevy_naadf/assets/test/oasis_hard_cover.vox` (LFS-tracked)

There is **no upload step for `.cvox`** anywhere in the workflow.

#### Wasm upload (for context)
`.github/workflows/deploy-cloudflare.yml:170-176`:
```yaml
- name: Upload WASM to R2
  run: |
    WASM_FILE=$(ls crates/bevy_naadf/dist/*_bg.wasm)
    npx wrangler r2 object put bevy-naadf-assets/bevy-naadf.wasm \
      --file "$WASM_FILE" --remote
```

### A.3 Runtime URL / fetch path in web bundle

**File:** `crates/bevy_naadf/src/voxel/web_vox.rs:45-46`
```rust
const DEFAULT_VOX_URL: &str =
    "https://bevy-naadf-assets.yura415.workers.dev/models/oasis_hard_cover.vox";
```

This constant is consumed by `resolve_startup_vox_url()` at `web_vox.rs:151-166`:
- If `window.location.search` contains `?vox=<url>`, that URL wins.
- Otherwise falls back to `DEFAULT_VOX_URL`.

`resolve_startup_vox_url` is called from `startup_fetch_default_vox` at
`web_vox.rs:300`, which is registered as a Bevy `Startup` system in
`lib.rs:792`.

The R2 proxy worker (`workers/r2-proxy/src/index.js:7-37`) forwards all keys
from the `bevy-naadf-assets` bucket with CORS headers — no extension allowlist,
no MIME-type restriction on what keys can be served. The worker uses
`object.writeHttpMetadata(headers)` which passes through whatever R2 stored.

---

## B. Web runtime fetch (dispatch coverage)

### B.1 Async-wasm path through parse_voxel_bytes

Confirmed via code trace (no cfg gaps):

1. `startup_fetch_default_vox` (`web_vox.rs:284`) kicks off `fetch_vox_bytes(url)`.
2. Bytes land in `PENDING_VOX_BYTES` via `submit_pending_bytes` (`web_vox.rs:72`).
3. `apply_pending_vox` (`web_vox.rs:361`) — Stage 1 moves bytes to `QUEUED_FOR_INSTALL`.
4. Stage 2 (next frame) calls `spawn_wasm_vox_parse` (`web_vox.rs:437`).
5. `spawn_wasm_vox_parse` (`web_vox.rs:437-455`) calls `rayon::spawn` with:
   ```rust
   let result = match crate::voxel::grid::parse_to_imported_vox(&bytes) {
   ```
   (`web_vox.rs:441`)
6. `parse_to_imported_vox` (`grid.rs:502-516`) — **post-`05-impl`** this is:
   ```rust
   pub fn parse_to_imported_vox(bytes: &[u8]) -> Result<vox_import::ImportedVox, String> {
       crate::voxel::voxel_dispatch::parse_voxel_bytes(bytes).map_err(|e| e.to_string())
   }
   ```
7. `voxel_dispatch::parse_voxel_bytes` peeks the first 4 bytes and routes to
   `vox_import::parse_vox_bytes` (magic `b"VOX "`) or
   `cvox_import::parse_cvox_bytes` (magic `b"PK\x03\x04"`).

**Verdict:** The runtime fetch path already supports `.cvox` bytes end-to-end.
If the R2 bucket serves `.cvox` bytes at the URL `DEFAULT_VOX_URL` points to,
the wasm runtime will parse and install them correctly via magic-byte dispatch —
no wasm-side code change needed.

This applies equally to drag-and-drop on web (also flows through the same
`submit_pending_bytes` → `parse_to_imported_vox` path, as designed in D4/A9).

### B.2 Web drag-and-drop

**File:** `crates/bevy_naadf/src/voxel/web_vox.rs:234-271`

The `drop` closure at `web_vox.rs:234` reads the dropped file via
`file.array_buffer()`, calls `submit_pending_bytes(bytes, ...)` — **no extension
filter**. The closure logs `file.name()` for diagnostics only. The raw bytes go
straight to the two-stage inbox. This matches the design's A9 assumption:
"drag-and-drop on web doesn't need extension filtering at the JS layer — the
magic-byte dispatch rejects non-voxel files cleanly."

Confirmed: dropping a `.cvox` onto the web build today (post-`05-impl`) will
parse it via the dispatch and install it. **No code change needed for web
drag-drop.**

---

## C. Web e2e tests

**File:** `e2e/tests/vox-loading.spec.ts`

The `vox-loading.spec.ts` file has one `test.describe.serial("Web .vox loading")`
block with two tests:

1. **"captures skybox baseline via ?skybox=1"** — does not touch the `.vox` asset at all;
   captures a pure-sky frame for the SSIM baseline. No asset URL involved.

2. **"startup-fetches and installs the default .vox…"** — at line 335:
   ```typescript
   await page.goto("/?vox=/test-fixtures/oasis_hard_cover.vox", {
   ```
   The `?vox=<url>` override bypasses `DEFAULT_VOX_URL` and loads
   `oasis_hard_cover.vox` from the local `serve.mjs` test-fixtures server
   (rooted at `crates/bevy_naadf/assets/test/`). The test then waits for
   the log line `"NAADF .vox loaded from"` (`vox-loading.spec.ts:317`), which
   is emitted format-agnostically by `grid.rs:536` (`install_imported_vox` uses
   `source_label`, not a format-specific string).

**What breaks if the URL changes to `.cvox`:**
- `vox-loading.spec.ts:335` must change from
  `/?vox=/test-fixtures/oasis_hard_cover.vox` to
  `/?vox=/test-fixtures/oasis.cvox`.
- `e2e/serve.mjs:26-43` MIME map must add `".cvox": "application/octet-stream"`
  (currently absent; `.vox` has an entry at line 42 but `.cvox` does not — the
  fallback is `"application/octet-stream"` via the `|| "application/octet-stream"` at
  line 90, so the serve will still work in practice, but it's better to make it
  explicit).
- The asset `crates/bevy_naadf/assets/test/oasis.cvox` must be present on disk
  — **it already is** (6.8 MB, committed directly to git without LFS).
- The log-line sentinel `"NAADF .vox loaded from"` at `vox-loading.spec.ts:317`
  does NOT need to change — `install_imported_vox` emits this string for any
  format (the string is a format-agnostic label baked into `grid.rs:536`).
- The SSIM and per-channel-spread assertions (`vox-loading.spec.ts:424-468`)
  apply to whatever the canvas renders — they are visually robust and will
  work with the `oasis.cvox` content (the Oasis model geometry). These do not
  need to change.

---

## D. Risk surface

### D.1 Hardcoded .vox references (grep results, every site)

The following table covers every `.vox` / `oasis_hard_cover` reference outside
`voxel_dispatch.rs`, `cvox_import.rs`, and `vox_import.rs`. References that are
in log-message strings (not logic) are marked accordingly.

| file:line | context | needs change? |
|---|---|---|
| `crates/bevy_naadf/src/voxel/web_vox.rs:45-46` | `DEFAULT_VOX_URL` — hardcoded R2 URL pointing at `models/oasis_hard_cover.vox` | **YES** — change to `models/oasis.cvox` (after CI upload step is added) |
| `.github/workflows/deploy-cloudflare.yml:182-190` | CI `Upload default .vox to R2` step — source is `assets/test/oasis_hard_cover.vox`, destination key `models/oasis_hard_cover.vox` | **YES** — add a new step to upload `oasis.cvox`; optionally keep the old step to avoid breaking users who have `.vox` bookmarked |
| `e2e/tests/vox-loading.spec.ts:335` | `page.goto("/?vox=/test-fixtures/oasis_hard_cover.vox")` | **YES** — change to `oasis.cvox` if the intent is to e2e-test `.cvox` delivery |
| `e2e/serve.mjs:42` | `".vox": "application/octet-stream"` — MIME map has `.vox` but NOT `.cvox` | **Recommended** — add `".cvox": "application/octet-stream"` (fallback already works but explicit is cleaner) |
| `crates/bevy_naadf/src/voxel/grid.rs:536` | Log string `"NAADF .vox loaded from"` | **No** — format-agnostic; emitted for `.cvox` too; the e2e test sentinel matches this string |
| `crates/bevy_naadf/src/voxel/grid.rs:397,427,472` | Error fallback log strings `".vox load failed"` | **No** — log strings only; no logic |
| `crates/bevy_naadf/src/voxel/grid.rs:737,756` | Drag-drop info logs mentioning `.vox` | **No** — log strings only; already handle both formats |
| `crates/bevy_naadf/src/voxel/async_vox.rs:99,115,131,145` | Parse error/timeout log strings | **No** — log strings only |
| `crates/bevy_naadf/src/voxel/web_vox.rs:76,315,373` | Info/error log strings mentioning `.vox` | **No** — log strings only; dispatching format-agnostically |
| `crates/bevy_naadf/src/e2e/vox_e2e.rs:333,354,361,441,674,695` | E2e gate synthesises its own `.vox` file on disk for native tests | **No** — unrelated to web deploy; tests `.vox` path only |
| `crates/bevy_naadf/src/e2e/vox_web_parity.rs:337,367` | Native vox-web-parity gate log strings | **No** — native e2e gate, not web-facing |
| `crates/bevy_naadf/src/render/construction/mod.rs:1076,1085,1092,1099,1210` | Internal `.vox run` guard log strings + comments | **No** — internal debug guards, not file-path logic |
| `crates/bevy_naadf/src/render/construction/mod.rs:4053,4061,4288` | Unit tests loading `oasis_hard_cover.vox` as fixture | **No** — native Rust unit tests, not web or CI |
| `crates/bevy_naadf/src/e2e/oasis_edit_visual.rs:81,91` | `OASIS_VOX_FIXTURE_PATH` / fixture path for native e2e | **No** — native `--oasis-edit-visual` gate |
| `crates/bevy_naadf/src/e2e/small_edit_repro.rs:12` | Comment referencing `oasis_hard_cover.vox` | **No** — comment only |
| `crates/bevy_naadf/src/main.rs:17` | Doc-comment referencing `oasis.cvox` (C# side) | **No** — already updated in 05-impl |
| `.gitattributes:3` | `*.vox filter=lfs diff=lfs merge=lfs -text` | **Consider** — `*.cvox` has no LFS rule; `oasis.cvox` is 6.8 MB committed as a regular git blob. If `.cvox` files could be large enough to need LFS, add `*.cvox filter=lfs diff=lfs merge=lfs -text`. Currently not urgent (6.8 MB is manageable). |
| `crates/bevy_naadf/index.html:84` | Comment `spawn_wasm_vox_parse for off-main-thread .vox parsing` | **No** — comment only |

### D.2 Web-deploy config (HTML / JS / wrangler / MIME / cache)

**`workers/r2-proxy/src/index.js` (full file, lines 1-37):**
The R2 proxy worker has **no extension allowlist** and **no MIME-type restrictions**.
It uses `object.writeHttpMetadata(headers)` to pass through whatever metadata
R2 stores at upload time. Wrangler's `r2 object put` does not set a custom
`Content-Type` header by default — it infers from the file extension (`.cvox` would
likely resolve to `application/octet-stream`, same as `.vox`). The CORS and CORP
headers are set unconditionally for all keys.

**Verdict: No allowlist or content-type rule would block `.cvox` in the R2 proxy.**

**`workers/r2-proxy/wrangler.toml` (full file, lines 1-14):**
No key-pattern restrictions. Bucket binding is `bevy-naadf-assets` with no
path prefix or allowlist.

**`crates/bevy_naadf/_headers` (Cloudflare Pages headers):**
Rules are path-pattern-based (`/*`, `/*.js`, `/*.wasm`, `/src/assets/*`). The
`.cvox` model is **not served from Cloudflare Pages** — it comes from R2 via the
proxy worker. Pages headers are irrelevant to the model fetch.

**`e2e/serve.mjs:26-43` (MIME map):**
`.cvox` is not in the map. The fallback at line 90 (`MIME_TYPES[ext] || "application/octet-stream"`)
means `.cvox` files **will** be served by the local test server with
`application/octet-stream` — this is functionally correct. However, a missing
explicit entry is a readability gap.

**Conclusion: no web-deploy config file blocks `.cvox` delivery. The gap is
an absent upload step and a stale URL constant, not a content-type or routing
restriction.**

---

## Proposed change surface (read-only recommendation)

### Asset shipping (upload)

- **`.github/workflows/deploy-cloudflare.yml`** — Add a new step after the
  existing "Upload default .vox to R2" step:
  - Source: `crates/bevy_naadf/assets/test/oasis.cvox`
  - R2 key: `bevy-naadf-assets/models/oasis.cvox`
  - No `lfs: true` change needed: `oasis.cvox` is not LFS-tracked (`check-attr`
    confirms `filter: unspecified` for `.cvox`). The regular `actions/checkout@v4`
    step already checked it out.

### Runtime fetch URL

- **`crates/bevy_naadf/src/voxel/web_vox.rs:45-46`** — Change `DEFAULT_VOX_URL`
  from `…/models/oasis_hard_cover.vox` to `…/models/oasis.cvox`.
  No other logic change required — `resolve_startup_vox_url` just returns the string.

### Web e2e (Playwright)

- **`e2e/tests/vox-loading.spec.ts:335`** — Change the `?vox=` query param from
  `/test-fixtures/oasis_hard_cover.vox` to `/test-fixtures/oasis.cvox`.
  The asset file exists at `crates/bevy_naadf/assets/test/oasis.cvox`.
  The log-line sentinel `"NAADF .vox loaded from"` (`vox-loading.spec.ts:317`)
  does NOT need to change — `install_imported_vox` emits it for any format.

- **`e2e/serve.mjs:26-43`** — Add `".cvox": "application/octet-stream"` to the
  MIME map alongside `.vox`. Functional impact is zero (the fallback covers it),
  but explicit is better.

### Git LFS (optional, not strictly required)

- **`.gitattributes`** — Optionally add `*.cvox filter=lfs diff=lfs merge=lfs -text`
  to keep `.cvox` files out of the git object store. Currently `oasis.cvox` is
  committed directly at 6.8 MB, which is small enough to leave untracked by LFS.
  If the architect adds this rule, `oasis.cvox` must be migrated to LFS
  (`git lfs migrate import --include="*.cvox"`) — the CI checkout already has
  `lfs: true`, so no CI change is needed for this.

### No code changes needed

- `voxel/web_vox.rs` drag-drop path — already format-agnostic.
- `voxel/grid.rs` parse shim — already the dispatch shim post-`05-impl`.
- `voxel_dispatch.rs`, `cvox_import.rs` — already fully wired.
- `workers/r2-proxy/src/index.js` — no restrictions to remove.
- Log-message strings mentioning `.vox` — cosmetic; not load-bearing.

---

## Borderline calls

**`.gitattributes` LFS rule for `*.cvox`:** The file is small (6.8 MB) and already
committed to git as a regular blob. Adding an LFS rule now would require a
`git lfs migrate import` to convert the existing object. The LFS rule for `*.vox`
was necessary because `oasis_hard_cover.vox` is 85 MB. At 6.8 MB, LFS for `.cvox`
is nice-to-have but not urgent. Flips from "not applicable" to "needed" if future
`.cvox` files are large (e.g. if the user exports large worlds to `.cvox`).

**Keeping the `.vox` upload step in CI:** The user's directive says "uploads and
loads cvox in github ci" — this likely means the `.vox` step can be removed or
kept in parallel. If the live web deployment URL switches to `.cvox`, the R2 key
`models/oasis_hard_cover.vox` becomes dead storage. Whether to remove the old
upload step is an operational call the architect should flag.

**`e2e/tests/vox-loading.spec.ts` — whether to switch the fixture or add a second
test:** The spec currently tests the `.vox` flow. Switching the fixture to `.cvox`
is the minimal change; adding a second `it()` block that tests `.cvox` would be
more thorough but is more work. Minimal switch is sufficient because the per-channel
and SSIM assertions are format-agnostic (they test visual output, not format identity).

---

## Files / line ranges read

- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/01-context.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/03-design.md` (full — D5 at lines 649-665)
- `/mnt/archive4/DEV/bevy-naadf/docs/orchestrate/oasis-vox-instance-count/05-impl.md` (full)
- `/mnt/archive4/DEV/bevy-naadf/.github/workflows/deploy-cloudflare.yml` (full, 204 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/web_vox.rs` (lines 1-472)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/grid.rs` (lines 525-556)
- `/mnt/archive4/DEV/bevy-naadf/workers/r2-proxy/src/index.js` (full, 37 lines)
- `/mnt/archive4/DEV/bevy-naadf/workers/r2-proxy/wrangler.toml` (full, 14 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/_headers` (full, 20 lines)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/index.html` (full, 193 lines)
- `/mnt/archive4/DEV/bevy-naadf/e2e/tests/vox-loading.spec.ts` (full, 473 lines)
- `/mnt/archive4/DEV/bevy-naadf/e2e/tests/wasm-smoke.spec.ts` (full, 89 lines)
- `/mnt/archive4/DEV/bevy-naadf/e2e/serve.mjs` (full, 103 lines)
- `/mnt/archive4/DEV/bevy-naadf/e2e/playwright.config.ts` (full, 61 lines)
- `/mnt/archive4/DEV/bevy-naadf/.gitattributes` (full, 4 lines)
- `git show 746b196 --stat` + message (LFS + deploy shape)
- `git lfs ls-files` (confirmed `oasis_hard_cover.vox` is LFS-tracked; `oasis.cvox` is not)
- `git check-attr filter` (confirmed `oasis.cvox` has no LFS attribute)
- `git ls-files --error-unmatch crates/bevy_naadf/assets/test/oasis.cvox` (confirmed in-tree)
- `find … -name "*.vox" -o -name "*.cvox"` (found both files on disk)
- `wc -c oasis.cvox` = 6791493; `wc -c oasis_hard_cover.vox` = 84911723
- `git log --oneline -5 -- crates/bevy_naadf/assets/test/oasis.cvox` (commit `b087a27`)
- Grep workspace for `.vox`, `oasis_hard_cover`, `DEFAULT_VOX_URL`, `workers.dev`, `oasis.cvox` (full workspace, excluding worktrees + target/)
