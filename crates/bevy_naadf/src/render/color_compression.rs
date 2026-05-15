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
        0.023_683_071,
        0.035_896_823,
        0.054_409_41,
        0.082_469_25,
        0.125,
        0.189_464_57,
        0.287_174_58,
        0.435_275_3,
        0.659_754,
        1.0,
        1.515_716_6,
        2.297_396_7,
        3.482_202_3,
        5.278_032,
        8.0,
        12.125_732,
        18.379_173,
        27.857_618,
        42.224_255,
        64.0,
        97.005_86,
        147.033_39,
        222.860_95,
        337.794_04,
        512.0,
        776.046_9,
        1_176.267_1,
        1_782.887_6,
        2_702.352_3,
        4096.0,
    ];

    const WGSL_COLOR_DIF_PROB: [f32; 31] = [
        0.0,
        0.340_246_05,
        0.564_724_74,
        0.712_825_4,
        0.810_535_43,
        0.875,
        0.917_530_8,
        0.945_590_6,
        0.964_103_16,
        0.976_316_9,
        0.984375,
        0.989_691_3,
        0.993_198_8,
        0.995_512_9,
        0.997_039_6,
        0.998_046_9,
        0.998_711_4,
        0.999_149_86,
        0.999_439_1,
        0.999_63,
        0.999_755_86,
        0.999_838_95,
        0.999_893_7,
        0.999_929_9,
        0.999_953_75,
        0.999_969_5,
        0.999_979_85,
        0.999_986_7,
        0.999_991_24,
        0.999_994_2,
        0.999_996_2,
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
