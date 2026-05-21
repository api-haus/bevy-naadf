## Scout pre-land (D7 step 0)

**Date**: 2026-05-21
**Author**: scout implementor (pre-land ahead of D7 main implementor)

### What was added

Two surgical changes to `crates/bevy_naadf/src/lib.rs`:

1. **`#[derive(PartialEq)]`** added to `GiSettings` at `lib.rs:109`.
   - Previous derive: `#[derive(Clone, Copy, Debug)]`
   - After: `#[derive(Clone, Copy, Debug, PartialEq)]`
   - All fields (`u32`, `f32`, `bool`) implement `PartialEq`; the derive is valid and cost-free.

2. **`impl GiSettings { pub const DEFAULTS: GiSettings = … }` block** inserted at `lib.rs:188–214` (immediately before the existing `impl Default for GiSettings`).
   - 19 fields, values identical to the existing `Default` impl body — single source of truth per architect §2 F2.
   - `sun_shadow_taps: 1` included (the architect's §2 F2 snippet listed 18 fields but omitted this one; cross-checked against the `Default` impl at the time of edit — all 19 fields present in both `DEFAULTS` and `default()`).

### File:line refs

- `crates/bevy_naadf/src/lib.rs:109` — `#[derive(Clone, Copy, Debug, PartialEq)]` on `GiSettings`
- `crates/bevy_naadf/src/lib.rs:188` — `impl GiSettings {` block start
- `crates/bevy_naadf/src/lib.rs:194` — `pub const DEFAULTS: GiSettings = GiSettings { … };`

### Build / test status

- `cargo build --workspace` — **pass** (42.7 s)
- `cargo test --workspace --lib` — **pass** (180 passed, 1 ignored, 5.69 s)

### Deviation from architect's plan

None. The architect's §2 F2 snippet omitted `sun_shadow_taps` from the `DEFAULTS` literal (likely a copy-paste elision — the field exists in the struct and `Default` impl). The scout added it to keep `DEFAULTS == GiSettings::default()` structurally complete. No other deviation.

### Notes for D7 main implementor

- `GiSettings::DEFAULTS` is now live on `main`. D2's KNOBS table can reference it immediately.
- The full D7 Step 2 move (relocating `GiSettings` to `settings/canonical.rs`) still needs to happen; this pre-land only adds the `const` and `PartialEq` in-place.
- The `Default` impl body still duplicates the field values from `DEFAULTS`; D7 Step 2 collapses `Default::default()` to `Self::DEFAULTS` when it moves the struct.
