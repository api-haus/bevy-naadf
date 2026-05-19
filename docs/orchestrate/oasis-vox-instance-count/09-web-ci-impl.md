# 09-web-ci-impl — web + CI deploy switched to .cvox

## general-purpose implementer findings (2026-05-19)

## Summary

Applied all 5 mechanical edits from the audit (`08-web-ci-audit.md`): added a
new CI upload step for `oasis.cvox` to the `bevy-naadf-assets` R2 bucket,
switched `DEFAULT_VOX_URL` in `web_vox.rs` from `oasis_hard_cover.vox` to
`oasis.cvox`, changed the Playwright e2e fixture URL to `/test-fixtures/oasis.cvox`,
added `.cvox` to the `serve.mjs` MIME map, and added a `*.cvox` LFS rule to
`.gitattributes`. Both verification gates pass: `cargo build --workspace` is
clean and `cargo test --workspace --lib` reports 200 passed / 1 ignored —
exactly the baseline from `05-impl`. Wasm target build (`cargo check --target
wasm32-unknown-unknown -p bevy-naadf`) also passes cleanly (pre-existing
warnings only).

## Edits applied (with before/after)

### Edit 1 — `.github/workflows/deploy-cloudflare.yml` (lines 178-204 area)

Added a new "Upload default .cvox to R2" step immediately after the existing
".vox" step. The `.vox` step's comment was also updated to note it is now kept
as a fallback.

Before (lines 178-190):
```yaml
      # Upload the default .vox model the web build streams on startup. Served
      # by the same R2 proxy worker as the wasm (cross-origin allowed), keyed
      # under `models/`. The URL is hard-coded in
      # `crates/bevy_naadf/src/voxel/web_vox.rs::DEFAULT_VOX_URL`.
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

After (lines 178-204):
```yaml
      # Upload the default .vox model the web build streams on startup. Served
      # by the same R2 proxy worker as the wasm (cross-origin allowed), keyed
      # under `models/`. Kept as a fallback / alternative asset; the live
      # DEFAULT_VOX_URL now points at oasis.cvox instead.
      - name: Upload default .vox to R2
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
        run: |
          npx wrangler r2 object put \
            bevy-naadf-assets/models/oasis_hard_cover.vox \
            --file crates/bevy_naadf/assets/test/oasis_hard_cover.vox \
            --remote

      # Upload the default .cvox model — the new web default fetched on startup.
      # oasis.cvox is committed directly to git (not LFS-tracked; 6.8 MB), so
      # the regular checkout above already materialises it — no lfs:true change
      # needed. DEFAULT_VOX_URL in web_vox.rs now points at this R2 key.
      - name: Upload default .cvox to R2
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
        run: |
          npx wrangler r2 object put \
            bevy-naadf-assets/models/oasis.cvox \
            --file crates/bevy_naadf/assets/test/oasis.cvox \
            --remote
```

### Edit 2 — `crates/bevy_naadf/src/voxel/web_vox.rs:41-46`

Before:
```rust
/// R2 key + URL for the default `.vox` model fetched on startup. The R2 proxy
/// worker (`workers/r2-proxy/src/index.js`) serves any key under the
/// `bevy-naadf-assets` bucket with `Cross-Origin-Resource-Policy: cross-origin`
/// so this cross-origin fetch succeeds from the Pages-served HTML.
const DEFAULT_VOX_URL: &str =
    "https://bevy-naadf-assets.yura415.workers.dev/models/oasis_hard_cover.vox";
```

After:
```rust
/// R2 key + URL for the default voxel model fetched on startup. The R2 proxy
/// worker (`workers/r2-proxy/src/index.js`) serves any key under the
/// `bevy-naadf-assets` bucket with `Cross-Origin-Resource-Policy: cross-origin`
/// so this cross-origin fetch succeeds from the Pages-served HTML.
/// Format dispatch is magic-byte-based (`voxel_dispatch::parse_voxel_bytes`),
/// so `.cvox` bytes are handled transparently by the same fetch → parse path.
const DEFAULT_VOX_URL: &str =
    "https://bevy-naadf-assets.yura415.workers.dev/models/oasis.cvox";
```

### Edit 3 — `e2e/tests/vox-loading.spec.ts:335`

Before:
```typescript
    await page.goto("/?vox=/test-fixtures/oasis_hard_cover.vox", {
```

After:
```typescript
    await page.goto("/?vox=/test-fixtures/oasis.cvox", {
```

### Edit 4 — `e2e/serve.mjs:42-43`

Before:
```js
  ".vox": "application/octet-stream",
};
```

After:
```js
  ".vox": "application/octet-stream",
  ".cvox": "application/octet-stream",
};
```

### Edit 5 — `.gitattributes:3-4`

Before:
```gitattributes
*.vox filter=lfs diff=lfs merge=lfs -text
```

After:
```gitattributes
*.vox filter=lfs diff=lfs merge=lfs -text
*.cvox filter=lfs diff=lfs merge=lfs -text
```

---

## D5 reversal

Design decision D5 in `03-design.md` (lines 649-665) explicitly kept the web
default URL pointing at the `.vox` asset, reasoning that "switching the default
web asset is a separate operational concern" and was out of scope for the
dispatch design. D5 noted the exact flip condition: "Explicit user directive to
switch the default." The user has now issued that directive verbatim. This
5-edit implementation performs the reversal without touching any runtime
dispatch logic — the wasm path already routes through `parse_voxel_bytes` per
`05-impl` (confirmed in audit section B.1), so only the asset-shipping
configuration needed to change.

---

## Verification gate results

- **`cargo build --workspace`**: PASS — `Finished dev profile in 0.63s`, 0
  crates compiled (fully cached; the only Rust change was a string constant in
  `web_vox.rs`, which does not force a full rebuild after the previous build).
- **`cargo test --workspace --lib`**: PASS — 200 passed, 1 ignored, 0 failed
  (2 suites, 5.20s). Exactly matches the `05-impl` baseline.
- **`cargo check --target wasm32-unknown-unknown -p bevy-naadf`**: PASS —
  `Finished dev profile in 41.01s`. Pre-existing warnings only (2 unused-import
  warnings that existed before this work). No errors. `trunk` is installed
  locally (`/home/midori/.cargo/bin/trunk`); `wasm32-unknown-unknown` target is
  installed; the optional wasm gate was worth running and passed.
- **`cargo run --bin bevy-naadf`**: NOT run. Forbidden as agent verification
  step per project CLAUDE.md.
- **Playwright e2e**: NOT run. Headed-only per user memory rule
  `playwright-e2e-must-be-headed`; this is the user's verification surface.
- **`wasm-pack`**: NOT installed or invoked.

---

## Test fixture mechanism for Edit 3

`e2e/serve.mjs` resolves `/test-fixtures/<name>` requests against
`crates/bevy_naadf/assets/test/` (lines 20-23). The file
`crates/bevy_naadf/assets/test/oasis.cvox` (6.8 MB, committed directly to git
at commit `b087a27`) is already present on disk — confirmed with
`find /mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/assets -name "*.cvox"`.
No copy, symlink, or path adjustment was needed. Changing the spec URL from
`/test-fixtures/oasis_hard_cover.vox` to `/test-fixtures/oasis.cvox` is
sufficient because `serve.mjs` resolves the request to
`crates/bevy_naadf/assets/test/oasis.cvox`, which exists. The log-line sentinel
at `vox-loading.spec.ts:317` (`"NAADF .vox loaded from"`) is unchanged — the
audit confirmed it is format-agnostic.

---

## Audit gaps surfaced

One site observed that the audit's D.1 table lists but marks "No":

- `crates/bevy_naadf/src/render/construction/mod.rs:4053,4061,4288` — unit
  tests that load `oasis_hard_cover.vox` as a fixture. These are native Rust
  unit tests that test the `.vox` import path specifically. They are not
  web-facing and are intentionally kept testing the `.vox` format. No edit
  needed or made.

One site the audit did not explicitly enumerate:

- `e2e/serve.mjs:55` — the comment at line 55 reads "the 85 MB Oasis .vox"
  (referring to `oasis_hard_cover.vox`). This is a comment only, not logic. The
  `.cvox` file served for the new default e2e test is 6.8 MB. The comment could
  be updated for accuracy, but it describes the `.vox` fixture in the context of
  explaining why fixtures live outside `dist/` — still true for `oasis_hard_cover.vox`,
  which remains in the test directory. No edit made (audit constraint: only the
  5 specified edits).

---

## Manual-QA hand-off for the user

1. **CI deploy**: push to `main` (or trigger `workflow_dispatch`). The updated
   `deploy-cloudflare.yml` will run both the "Upload default .vox to R2" step
   and the new "Upload default .cvox to R2" step. Verify in the Actions log
   that both steps succeed and `models/oasis.cvox` appears as an R2 object in
   the `bevy-naadf-assets` bucket.

2. **Live web build**: visit the deployed Pages URL (no `?vox=` override needed
   — the new `DEFAULT_VOX_URL` will auto-fetch `oasis.cvox` on startup). The
   web build should load the Oasis `.cvox` model and render exactly 4 modulo-
   wrapped copies along X and Z axes (versus ~2.5 copies that the old
   `oasis_hard_cover.vox` produced, as described in `05-impl.md:240-244`).

3. **Web e2e (headed)**: from the `e2e/` directory, run:
   ```sh
   cd /mnt/archive4/DEV/bevy-naadf/e2e
   npx playwright test vox-loading
   ```
   (or check `e2e/package.json` for the project-specific test command — likely
   `npm test` or a `just test-wasm` recipe). The `vox-loading.spec.ts` test now
   loads `oasis.cvox` via the `?vox=/test-fixtures/oasis.cvox` query param.
   Recall: web e2e is headed-only; run on a display with a Chrome/Chromium
   browser that has WebGPU enabled.

4. **Native smoke (optional, confirms same file)**: the native CLI uses the same
   `oasis.cvox` file via the same dispatch path:
   ```sh
   cargo run --release --bin bevy-naadf -- \
       --vox crates/bevy_naadf/assets/test/oasis.cvox
   ```
   Should render 4 × 4 Oasis tiles. This is the user's visual verification step
   (not an agent verification step).

---

## Files touched

- `/mnt/archive4/DEV/bevy-naadf/.github/workflows/deploy-cloudflare.yml` —
  lines 178-204 (updated comment on `.vox` step; added new `.cvox` upload step)
- `/mnt/archive4/DEV/bevy-naadf/crates/bevy_naadf/src/voxel/web_vox.rs` —
  lines 41-47 (`DEFAULT_VOX_URL` constant + doc-comment)
- `/mnt/archive4/DEV/bevy-naadf/e2e/tests/vox-loading.spec.ts` — line 335
  (`?vox=` fixture URL)
- `/mnt/archive4/DEV/bevy-naadf/e2e/serve.mjs` — line 43 (`.cvox` MIME entry
  added after `.vox` entry)
- `/mnt/archive4/DEV/bevy-naadf/.gitattributes` — line 4 (`*.cvox` LFS rule
  added after `*.vox` rule)
