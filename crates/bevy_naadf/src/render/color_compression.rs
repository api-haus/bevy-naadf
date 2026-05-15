//! CPU-side recomputation of the `commonColorCompression.fxh` exponential
//! colour tables, and the guard test that keeps the hard-coded WGSL literals
//! honest (`09-design-b.md` §5.3, §12 #4).
//!
//! The HLSL builds `COLORS[32]` / `COLOR_DIF_PROB[31]` from `pow()` const-
//! expressions; WGSL `const` arrays cannot hold `pow()` results, so
//! `assets/shaders/color_compression.wgsl` hard-codes the 32 + 31 computed f32
//! literals. This module recomputes them from the source formula and the
//! `#[test]` below asserts the WGSL literals match — the same discipline as
//! `gpu_types.rs`'s compile-time size asserts.
//!
//! There is no GPU resource here — the tables ship as WGSL literals (option (a)
//! in `09-design-b.md` §5.3, chosen over uploading a uniform). This module is
//! pure CPU-side bookkeeping + the test.

/// `COLOR_START = 1 / 64` (HLSL `commonColorCompression.fxh:8`).
pub const COLOR_START: f64 = 1.0 / 64.0;

/// `COLOR_EXP = pow(2, 0.6)` (HLSL `commonColorCompression.fxh:7`). Computed at
/// runtime — `f64::powf` is not `const`-callable, and computing it here (rather
/// than hard-coding the f64 literal) keeps it exactly the value the WGSL
/// literals were generated from.
pub fn color_exp() -> f64 {
    2.0_f64.powf(0.6)
}

/// Recompute the 32-entry `COLORS` LUT (HLSL `static const float COLORS[32]`):
/// `COLORS[0] = 0`, `COLORS[i] = COLOR_START * COLOR_EXP^(i-1)` for `i in 1..=31`.
pub fn colors() -> [f32; 32] {
    let exp = color_exp();
    let mut out = [0.0_f32; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = if i == 0 {
            0.0
        } else {
            (COLOR_START * exp.powi(i as i32 - 1)) as f32
        };
    }
    out
}

/// Recompute the 31-entry `COLOR_DIF_PROB` LUT (HLSL
/// `static const float COLOR_DIF_PROB[31]`):
/// `COLOR_DIF_PROB[i] = 1 - COLOR_EXP^(-i)` for `i in 0..=30`.
pub fn color_dif_prob() -> [f32; 31] {
    let exp = color_exp();
    let mut out = [0.0_f32; 31];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (1.0 - 1.0 / exp.powi(i as i32)) as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hard-coded literals in `assets/shaders/color_compression.wgsl` —
    /// kept here so the guard test catches any drift between the WGSL file and
    /// the source formula. If you regenerate the WGSL literals, update these
    /// (or vice-versa — they MUST agree, that is the whole point of this test).
    const WGSL_COLORS: [f32; 32] = [
        0.0,
        0.015625,
        0.02368307113647461,
        0.035896822810173035,
        0.05440941080451012,
        0.08246924728155136,
        0.125,
        0.18946456909179688,
        0.2871745824813843,
        0.43527528643608093,
        0.6597539782524109,
        1.0,
        1.515716552734375,
        2.297396659851074,
        3.4822022914886475,
        5.278031826019287,
        8.0,
        12.125732421875,
        18.379173278808594,
        27.85761833190918,
        42.2242546081543,
        64.0,
        97.005859375,
        147.03338623046875,
        222.86094665527344,
        337.7940368652344,
        512.0,
        776.046875,
        1176.26708984375,
        1782.8875732421875,
        2702.352294921875,
        4096.0,
    ];

    const WGSL_COLOR_DIF_PROB: [f32; 31] = [
        0.0,
        0.3402460515499115,
        0.5647247433662415,
        0.7128254175186157,
        0.8105354309082031,
        0.875,
        0.9175307750701904,
        0.945590615272522,
        0.9641031622886658,
        0.9763169288635254,
        0.984375,
        0.9896913170814514,
        0.993198812007904,
        0.9955129027366638,
        0.9970396161079407,
        0.998046875,
        0.9987114071846008,
        0.9991498589515686,
        0.9994391202926636,
        0.9996299743652344,
        0.999755859375,
        0.9998389482498169,
        0.9998937249183655,
        0.9999299049377441,
        0.9999537467956543,
        0.999969482421875,
        0.9999798536300659,
        0.9999867081642151,
        0.999991238117218,
        0.9999942183494568,
        0.9999961853027344,
    ];

    /// The recomputed tables must match the hard-coded WGSL literals exactly
    /// (`09-design-b.md` §12 #4 — the guard the design specifies).
    #[test]
    fn color_tables_match_wgsl() {
        let colors = colors();
        let prob = color_dif_prob();
        for i in 0..32 {
            assert_eq!(
                colors[i].to_bits(),
                WGSL_COLORS[i].to_bits(),
                "COLORS[{i}] drifted: recomputed {} != WGSL literal {}",
                colors[i],
                WGSL_COLORS[i],
            );
        }
        for i in 0..31 {
            assert_eq!(
                prob[i].to_bits(),
                WGSL_COLOR_DIF_PROB[i].to_bits(),
                "COLOR_DIF_PROB[{i}] drifted: recomputed {} != WGSL literal {}",
                prob[i],
                WGSL_COLOR_DIF_PROB[i],
            );
        }
    }

    /// Spot-check the source formula's anchor points: `COLORS[0] == 0`,
    /// `COLORS[1] == COLOR_START`, and the `COLOR_EXP^5 == 8` /
    /// `COLOR_EXP^10 == 1024`-ish doubling structure (`2^0.6` raised five times
    /// is `2^3 = 8`, so `COLORS[6] == COLOR_START * 8 == 0.125`).
    #[test]
    fn color_table_anchor_points() {
        let colors = colors();
        assert_eq!(colors[0], 0.0);
        assert_eq!(colors[1], COLOR_START as f32);
        assert_eq!(colors[6], 0.125);
        assert_eq!(colors[11], 1.0);
        assert_eq!(colors[31], 4096.0);
        let prob = color_dif_prob();
        assert_eq!(prob[0], 0.0);
    }
}
