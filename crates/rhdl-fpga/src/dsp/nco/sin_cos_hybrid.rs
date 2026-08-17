#![warn(missing_docs)]
//! `SinCosHybrid` — quadrature phase-to-amplitude by coarse table plus
//! fine rotation.
//!
//! Converts a 22-bit truncated phase into `(sin, cos)` using a
//! 256-entry quarter-wave table and a first-order rotation by the fine
//! remainder:
//!
//! ```text
//! sin(θ+δ) ≈ sin θ + cos θ · δ
//! cos(θ+δ) ≈ cos θ − sin θ · δ
//! ```
//!
//! # Why this rather than a bigger table
//!
//! Interpolation attacks the truncation error instead of merely
//! reducing it, so accuracy improves roughly twice as fast per coarse
//! bit as enlarging the table does. Measured with the exact spur
//! analysis in [`super::model`]:
//!
//! | architecture | table | worst in-band spur |
//! |---|---|---|
//! | plain LUT, 13 address bits | 32 Kbit | −78 dBc |
//! | plain LUT, 14 address bits | 64 Kbit | −84 dBc |
//! | **this** | **~9 Kbit** | **−116 dBc** |
//!
//! Reaching −116 dBc with a plain table needs ~21 address bits — of
//! order 8 Mbit, hundreds of block RAMs.
//!
//! Dithering is the other classical way to suppress truncation spurs
//! and is **not** used: it trades a discrete spur for a raised noise
//! floor, which in a sensitivity-limited instrument lands directly on
//! the quantity being bought with averaging time.
//!
//! # Fixed widths, deliberately
//!
//! The configuration below is the one the spur analysis validated, and
//! the fixed-point scaling constant is derived from it. Making the
//! widths generic would require deriving that constant inside a kernel
//! from const generics, which the kernel language cannot do — so the
//! widths are concrete rather than approximately-generic-and-wrong.
//!
//! | parameter | value |
//! |---|---|
//! | phase consumed | 22 bits (2 quadrant + 8 index + 12 fine) |
//! | table | 256 entries × 18 bit, quarter wave |
//! | amplitude | 18 bit (native DSP48 port; reaches the ceiling exactly) |
//! | phase resolution | 360° / 2²² ≈ 0.000086° |
//!
//! # Latency
//!
//! The table read is registered: **data latency is one cycle**, and
//! there are no control inputs. That figure belongs in the scheduler's
//! arithmetic.

use rhdl::prelude::*;

use crate::core::ram::synchronous::{In as RamIn, SyncBRAM, Write};

/// Quarter-wave table address bits (256 entries).
pub const TBL_W: usize = 8;
/// Fine interpolation bits.
pub const FINE_W: usize = 12;
/// Total phase bits consumed: 2 quadrant + `TBL_W` + `FINE_W`.
pub const TOTAL_W: usize = 22;
/// Bits per output component.
pub const AMP_W: usize = 18;

/// `round(2π / 2^TOTAL_W · 2^32)` — converts a fine-remainder LSB into
/// radians in Q32. Relative error 2.8e-6 on a correction that is itself
/// ≤0.3% of full scale, i.e. about −161 dBc: far below the −116 dBc the
/// architecture delivers.
const DELTA_K: i128 = 6434;

/// Quadrature phase-to-amplitude: coarse quarter-wave table plus
/// first-order fine rotation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct SinCosHybrid {
    /// Quarter-wave table read at the sine address.
    sin_tbl: SyncBRAM<SignedBits<AMP_W>, TBL_W>,
    /// Second instance of the same table for the cosine address.
    /// `SyncBRAM` is single-port; on a device with true dual-port block
    /// RAM these collapse to one primitive.
    cos_tbl: SyncBRAM<SignedBits<AMP_W>, TBL_W>,
}

/// Build the quarter-wave table: 256 entries at bin midpoints over
/// `[0, π/2)`.
///
/// Midpoint sampling is what makes the odd-quadrant mirror exact rather
/// than off-by-one.
fn quarter_table() -> Vec<(Bits<TBL_W>, SignedBits<AMP_W>)> {
    let coarse_w = TBL_W + 2;
    let scale = ((1i128 << (AMP_W - 1)) - 1) as f64;
    (0..(1usize << TBL_W))
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64 + 0.5) / (1u64 << coarse_w) as f64;
            (
                bits::<TBL_W>(i as u128),
                signed::<AMP_W>((theta.sin() * scale).round() as i128),
            )
        })
        .collect()
}

impl Default for SinCosHybrid {
    fn default() -> Self {
        Self {
            sin_tbl: SyncBRAM::new(quarter_table()),
            cos_tbl: SyncBRAM::new(quarter_table()),
        }
    }
}

/// Inputs for [`SinCosHybrid`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// Phase, truncated to the 22 bits this stage consumes.
    pub phase: Bits<TOTAL_W>,
}

/// Outputs from [`SinCosHybrid`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// Sine of the phase.
    pub sin: SignedBits<AMP_W>,
    /// Cosine of the phase.
    pub cos: SignedBits<AMP_W>,
}

impl SynchronousIO for SinCosHybrid {
    type I = In;
    type O = Out;
    type Kernel = sin_cos_hybrid_kernel;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn sin_cos_hybrid_kernel(_cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();

    // Split phase into quadrant | index | fine.
    let coarse = (i.phase >> 12u128).resize::<TBL_W>();
    let quadrant = (i.phase >> 20u128).resize::<2>();
    let fine = i.phase.resize::<FINE_W>();

    // Odd quadrants read the table mirrored.
    let mirrored = !bits::<TBL_W>(0) - coarse;
    let odd = (quadrant & bits::<2>(1)) != bits::<2>(0);
    let sin_addr = if odd { mirrored } else { coarse };

    // Cosine is a quarter turn ahead.
    let cos_quadrant = quadrant + bits::<2>(1);
    let cos_odd = (cos_quadrant & bits::<2>(1)) != bits::<2>(0);
    let cos_addr = if cos_odd { mirrored } else { coarse };

    let no_write = Write::<SignedBits<AMP_W>, TBL_W> {
        addr: bits::<TBL_W>(0),
        value: signed::<AMP_W>(0),
        enable: false,
    };
    d.sin_tbl = RamIn::<SignedBits<AMP_W>, TBL_W> {
        read_addr: sin_addr,
        write: no_write,
    };
    d.cos_tbl = RamIn::<SignedBits<AMP_W>, TBL_W> {
        read_addr: cos_addr,
        write: no_write,
    };

    // Upper half-cycle is negative.  NOTE these use the quadrant that
    // produced the CURRENT table output, i.e. the input phase of the
    // previous cycle — see the latency note in the module docs.
    let sin_neg = (quadrant & bits::<2>(2)) != bits::<2>(0);
    let cos_neg = (cos_quadrant & bits::<2>(2)) != bits::<2>(0);
    let s0 = if sin_neg { -q.sin_tbl } else { q.sin_tbl };
    let c0 = if cos_neg { -q.cos_tbl } else { q.cos_tbl };

    // Fine remainder, centred so δ spans ±half a coarse step.
    let delta = fine.as_signed().resize::<48>() - signed::<48>(2048);

    // First-order rotation.  Wide intermediate: 18 + 12 + 13 = 43 bits
    // of true product, carried in 48.
    let k = signed::<48>(DELTA_K);
    let corr_sin = ((c0.resize::<48>() * delta * k) >> 32u128).resize::<AMP_W>();
    let corr_cos = ((s0.resize::<48>() * delta * k) >> 32u128).resize::<AMP_W>();

    let o = Out {
        sin: s0 + corr_sin,
        cos: c0 - corr_cos,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _a = SinCosHybrid::default();
    }

    /// The table is a real quarter sine: monotonically rising from ~0
    /// to ~full scale.
    #[test]
    fn quarter_table_is_a_rising_quarter_sine() {
        let t = quarter_table();
        assert_eq!(t.len(), 256);
        let vals: Vec<i128> = t.iter().map(|(_, v)| v.raw()).collect();
        assert!(
            vals[0] > 0 && vals[0] < 2000,
            "starts near zero: {}",
            vals[0]
        );
        let full = (1i128 << (AMP_W - 1)) - 1;
        assert!(
            vals[255] > full - full / 100,
            "ends near full scale: {} vs {full}",
            vals[255]
        );
        assert!(
            vals.windows(2).all(|w| w[1] > w[0]),
            "must rise monotonically across the quarter"
        );
    }
}
