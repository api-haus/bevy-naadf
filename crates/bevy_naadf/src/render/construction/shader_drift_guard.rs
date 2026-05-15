//! Phase-C followups — inline-duplication drift guard for `bounds_common.wgsl`
//! helpers (concern #2 from `17-review-c.md`).
//!
//! The construction shaders inline copies of the `boundsCommon.fxh` helpers
//! (`MASK_*` constants, `cached_cell` workgroup-shared array,
//! `check_matching_bounds`, `add_bounds_voxels_or_blocks`, `compute_bounds_4`)
//! because Bevy's WGSL `#import` surface is unpredictable across naga versions.
//! The shader headers (`chunk_calc.wgsl:46` etc.) claim a const-guard test
//! `bounds_common_inline_matches_ref` enforces the copies are identical — but
//! the test never existed.
//!
//! This module ships that test. The four shaders involved:
//!
//! - `bounds_common.wgsl` — the canonical reference. Holds `MASK_*` constants,
//!   the `cached_cell` workgroup-shared array, and all three helper functions.
//! - `chunk_calc.wgsl` — full inline copy.
//! - `world_change.wgsl` — full inline copy.
//! - `bounds_calc.wgsl` — copies the `MASK_*` constants only (its 5-bit
//!   variants have their own helpers; the bounds_common 2-bit
//!   helpers do not apply).
//!
//! ## Strategy
//!
//! Each block of interest in the canonical shader is extracted by anchoring
//! on a unique start marker + an end marker. The same anchors are extracted
//! from each inline copy and compared after a normalisation pass that strips
//! pure whitespace differences (trailing whitespace, blank lines, and lines
//! that are *only* commentary). The body comparison enforces every
//! load-bearing line (constant value, function signature, control flow,
//! barrier point) survives intact across copies.

/// The four shader file paths (relative to the crate `src/` root, matching
/// how the existing `*_SHADER_SRC` constants resolve them).
pub const BOUNDS_COMMON_SRC: &str =
    include_str!("../../assets/shaders/bounds_common.wgsl");
pub const CHUNK_CALC_SRC: &str =
    include_str!("../../assets/shaders/chunk_calc.wgsl");
pub const WORLD_CHANGE_SRC: &str =
    include_str!("../../assets/shaders/world_change.wgsl");
pub const BOUNDS_CALC_SRC: &str =
    include_str!("../../assets/shaders/bounds_calc.wgsl");

#[cfg(test)]
/// Aggressive whitespace + comment normaliser: strip end-of-line `//` comments,
/// drop blank-only lines and pure-comment lines, collapse all runs of
/// whitespace to single spaces, and concatenate every code line into one
/// space-separated token stream. The result is a canonical form that captures
/// semantic content while ignoring every cosmetic difference (trailing
/// commas, multi-line vs single-line argument lists, end-of-line comments,
/// indentation).
fn normalise(s: &str) -> String {
    // First pass: strip comments + collect characters into a single string,
    // separating original whitespace runs by a single space.
    let mut flat = String::new();
    for line in s.lines() {
        let code_only = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        };
        let trimmed = code_only.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !flat.is_empty() {
            flat.push(' ');
        }
        for token in trimmed.split_whitespace() {
            flat.push_str(token);
            flat.push(' ');
        }
    }
    // Second pass: split into characters and emit a canonical separator-
    // sensitive stream — every WGSL punctuator gets a space on both sides
    // (so `cur_cell);` becomes `cur_cell ) ;`, matching how the same logical
    // call site is written multi-line as `cur_cell, );` → `cur_cell , ) ;`
    // once trailing commas are collapsed). Trailing commas before `)` are
    // dropped (WGSL accepts both forms).
    let punct: &[char] = &['(', ')', '{', '}', '[', ']', ',', ';', ':', '<', '>'];
    let mut spaced = String::with_capacity(flat.len() * 2);
    for ch in flat.chars() {
        if punct.contains(&ch) {
            spaced.push(' ');
            spaced.push(ch);
            spaced.push(' ');
        } else {
            spaced.push(ch);
        }
    }
    // Third pass: tokenise + drop trailing-comma-before-`)` pairs (they are
    // pure formatting). Also collapse `< <` / `> >` runs back into multi-char
    // tokens where they appear (e.g. `array<u32, 64>` stays meaningful, but
    // we don't need to reconstruct types — generic-angle-bracket sequences
    // are already split per char and we compare token-for-token, so the same
    // canonical & copy sources both produce the same tokenisation).
    let raw_tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut tokens: Vec<&str> = Vec::with_capacity(raw_tokens.len());
    for (i, t) in raw_tokens.iter().enumerate() {
        if *t == "," {
            // Drop if the next non-whitespace token is `)` or `}` (trailing
            // comma).
            if let Some(next) = raw_tokens.get(i + 1) {
                if *next == ")" || *next == "}" {
                    continue;
                }
            }
        }
        tokens.push(t);
    }
    tokens.join(" ")
}

#[cfg(test)]
/// Extract the substring from `src` starting at the first occurrence of
/// `start_anchor` and ending at the first occurrence of `end_anchor` after
/// that start. Both anchors are included in the returned slice. Returns
/// `None` if either anchor is missing.
fn extract_between<'a>(src: &'a str, start_anchor: &str, end_anchor: &str) -> Option<&'a str> {
    let start = src.find(start_anchor)?;
    let after = &src[start..];
    let end_rel = after.find(end_anchor)?;
    Some(&src[start..start + end_rel + end_anchor.len()])
}

#[cfg(test)]
/// Extract the `MASK_MX..MASK_PZ` const block. The block starts at
/// `const MASK_MX:` and ends at the line `const MASK_PZ:` (inclusive of the
/// `0x2Fu;` closing).
fn extract_masks(src: &str) -> Option<String> {
    extract_between(src, "const MASK_MX:", "const MASK_PZ: u32 = 0x2Fu;")
        .map(normalise)
}

#[cfg(test)]
/// Extract the `cached_cell` workgroup declaration. Single-line declaration —
/// anchor on the prefix + a `;` end.
fn extract_cached_cell(src: &str) -> Option<String> {
    // The exact declaration line is:
    //   var<workgroup> cached_cell: array<u32, 64>;
    // (whitespace before `array` is part of the canonical form).
    let needle = "var<workgroup> cached_cell: array<u32, 64>;";
    if src.contains(needle) {
        Some(needle.to_string() + "\n")
    } else {
        None
    }
}

#[cfg(test)]
/// Extract `check_matching_bounds`'s function body. Anchored at the signature
/// `fn check_matching_bounds(` and closed at the first `\n}\n` after the
/// signature (the function-close brace on its own line).
fn extract_check_matching_bounds(src: &str) -> Option<String> {
    extract_fn(src, "fn check_matching_bounds(")
}

#[cfg(test)]
/// Extract `add_bounds_voxels_or_blocks`'s function body. Same closing rule.
fn extract_add_bounds(src: &str) -> Option<String> {
    extract_fn(src, "fn add_bounds_voxels_or_blocks(")
}

#[cfg(test)]
/// Extract `compute_bounds_4`'s function body. Same closing rule.
fn extract_compute_bounds_4(src: &str) -> Option<String> {
    extract_fn(src, "fn compute_bounds_4(")
}

#[cfg(test)]
/// Extract a top-level `fn …{ … }` block starting at `signature_start`. The
/// extractor walks brace depth from the first `{` it finds after the signature
/// and stops at the matching `}`. Returns the normalised body so trivial
/// formatting drift does not trip the guard.
fn extract_fn(src: &str, signature_start: &str) -> Option<String> {
    let start = src.find(signature_start)?;
    let after = &src[start..];
    let brace_start = after.find('{')?;
    let mut depth: i32 = 0;
    let bytes = after.as_bytes();
    let mut end_rel = None;
    for (i, &b) in bytes.iter().enumerate().skip(brace_start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end_rel = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end_rel = end_rel?;
    Some(normalise(&src[start..start + end_rel]))
}

/// Result of the inline-duplication audit — collected per-shader so test
/// failures can surface every drift in one report rather than one at a time.
#[cfg(test)]
#[derive(Debug)]
struct DriftAudit {
    /// Items that the canonical shader has + every inline copy must match.
    items: Vec<DriftItem>,
}

#[cfg(test)]
#[derive(Debug)]
struct DriftItem {
    /// Human-readable name (`"MASK_*"`, `"compute_bounds_4"`, etc).
    name: &'static str,
    /// Normalised canonical text.
    canonical: String,
    /// Shaders required to carry this item, paired with their normalised text
    /// or `None` if the copy is absent (which is itself a drift).
    copies: Vec<(&'static str, Option<String>)>,
}

#[cfg(test)]
impl DriftItem {
    /// Compare every copy against `canonical`. Returns `None` if all match,
    /// or a list of mismatches `(shader_name, reason)`.
    fn check(&self) -> Result<(), Vec<(&'static str, String)>> {
        let mut errors = Vec::new();
        for (name, copy) in &self.copies {
            match copy {
                None => errors.push((*name, "section missing entirely".to_string())),
                Some(body) => {
                    if body != &self.canonical {
                        // Find the first token mismatch + a small surrounding
                        // context, so the failure report points at where the
                        // drift started.
                        let canonical_tokens: Vec<&str> =
                            self.canonical.split(' ').collect();
                        let copy_tokens: Vec<&str> = body.split(' ').collect();
                        let mut first_diff = None;
                        for (i, (a, b)) in canonical_tokens
                            .iter()
                            .zip(copy_tokens.iter())
                            .enumerate()
                        {
                            if a != b {
                                first_diff = Some(i);
                                break;
                            }
                        }
                        let first_diff = first_diff
                            .unwrap_or(canonical_tokens.len().min(copy_tokens.len()));
                        let lo = first_diff.saturating_sub(2);
                        let hi_can = (first_diff + 4).min(canonical_tokens.len());
                        let hi_cp = (first_diff + 4).min(copy_tokens.len());
                        let reason = format!(
                            "section drifted (normalised token streams differ). \
                             First diff at token {}; canonical context: {:?}; \
                             copy context: {:?}",
                            first_diff,
                            &canonical_tokens[lo..hi_can],
                            &copy_tokens[lo..hi_cp],
                        );
                        errors.push((*name, reason));
                    }
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Build the audit set for every duplicated item.
#[cfg(test)]
fn build_audit() -> DriftAudit {
    // ── MASK_* constants ──────────────────────────────────────────────────
    let mask_canonical =
        extract_masks(BOUNDS_COMMON_SRC).expect("canonical MASK_* not found");
    let mask_chunk = extract_masks(CHUNK_CALC_SRC);
    let mask_world_change = extract_masks(WORLD_CHANGE_SRC);
    let mask_bounds_calc = extract_masks(BOUNDS_CALC_SRC);

    // ── cached_cell ───────────────────────────────────────────────────────
    let cell_canonical =
        extract_cached_cell(BOUNDS_COMMON_SRC).expect("canonical cached_cell not found");
    let cell_chunk = extract_cached_cell(CHUNK_CALC_SRC);
    let cell_world_change = extract_cached_cell(WORLD_CHANGE_SRC);
    // `bounds_calc.wgsl` does NOT use the 4³ `cached_cell` (it has its own
    // 5-bit AADF chunk-AADF expansion algorithm — `bounds_calc.wgsl:127-145`
    // documents the W3 deviation). It is intentionally absent there.

    // ── check_matching_bounds ─────────────────────────────────────────────
    let cmb_canonical = extract_check_matching_bounds(BOUNDS_COMMON_SRC)
        .expect("canonical check_matching_bounds not found");
    let cmb_chunk = extract_check_matching_bounds(CHUNK_CALC_SRC);
    let cmb_world_change = extract_check_matching_bounds(WORLD_CHANGE_SRC);
    // Absent from bounds_calc by design.

    // ── add_bounds_voxels_or_blocks ───────────────────────────────────────
    let ab_canonical = extract_add_bounds(BOUNDS_COMMON_SRC)
        .expect("canonical add_bounds_voxels_or_blocks not found");
    let ab_chunk = extract_add_bounds(CHUNK_CALC_SRC);
    let ab_world_change = extract_add_bounds(WORLD_CHANGE_SRC);

    // ── compute_bounds_4 ──────────────────────────────────────────────────
    let cb_canonical = extract_compute_bounds_4(BOUNDS_COMMON_SRC)
        .expect("canonical compute_bounds_4 not found");
    let cb_chunk = extract_compute_bounds_4(CHUNK_CALC_SRC);
    let cb_world_change = extract_compute_bounds_4(WORLD_CHANGE_SRC);

    DriftAudit {
        items: vec![
            DriftItem {
                name: "MASK_MX..MASK_PZ",
                canonical: mask_canonical,
                copies: vec![
                    ("chunk_calc.wgsl", mask_chunk),
                    ("world_change.wgsl", mask_world_change),
                    ("bounds_calc.wgsl", mask_bounds_calc),
                ],
            },
            DriftItem {
                name: "cached_cell workgroup decl",
                canonical: cell_canonical,
                copies: vec![
                    ("chunk_calc.wgsl", cell_chunk),
                    ("world_change.wgsl", cell_world_change),
                ],
            },
            DriftItem {
                name: "check_matching_bounds",
                canonical: cmb_canonical,
                copies: vec![
                    ("chunk_calc.wgsl", cmb_chunk),
                    ("world_change.wgsl", cmb_world_change),
                ],
            },
            DriftItem {
                name: "add_bounds_voxels_or_blocks",
                canonical: ab_canonical,
                copies: vec![
                    ("chunk_calc.wgsl", ab_chunk),
                    ("world_change.wgsl", ab_world_change),
                ],
            },
            DriftItem {
                name: "compute_bounds_4",
                canonical: cb_canonical,
                copies: vec![
                    ("chunk_calc.wgsl", cb_chunk),
                    ("world_change.wgsl", cb_world_change),
                ],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The drift guard** — `chunk_calc.wgsl:46` / `world_change.wgsl` /
    /// `bounds_common.wgsl` headers all claim this test exists. It now does.
    ///
    /// Asserts every inline copy of `MASK_*`, `cached_cell`,
    /// `check_matching_bounds`, `add_bounds_voxels_or_blocks`, and
    /// `compute_bounds_4` matches the canonical `bounds_common.wgsl`
    /// definitions (after whitespace + pure-comment normalisation).
    #[test]
    fn bounds_common_inline_matches_ref() {
        let audit = build_audit();
        let mut failures: Vec<String> = Vec::new();
        for item in &audit.items {
            if let Err(errors) = item.check() {
                for (shader, reason) in errors {
                    failures.push(format!(
                        "[{}] inline {} drift in {}: {}",
                        item.name, item.name, shader, reason
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "bounds_common inline-duplication drift detected ({} items):\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }

    /// Sanity probe: the canonical extracts succeed (catch a future refactor
    /// that renames or restructures `bounds_common.wgsl` so the anchors miss).
    #[test]
    fn bounds_common_canonical_extractors_succeed() {
        assert!(extract_masks(BOUNDS_COMMON_SRC).is_some());
        assert!(extract_cached_cell(BOUNDS_COMMON_SRC).is_some());
        assert!(extract_check_matching_bounds(BOUNDS_COMMON_SRC).is_some());
        assert!(extract_add_bounds(BOUNDS_COMMON_SRC).is_some());
        assert!(extract_compute_bounds_4(BOUNDS_COMMON_SRC).is_some());
    }
}
