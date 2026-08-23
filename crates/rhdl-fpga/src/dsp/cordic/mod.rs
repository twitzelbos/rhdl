#![warn(missing_docs)]
//! CORDIC conversion between rectangular and polar samples.
//!
//! # Read this before instantiating one
//!
//! **On an FPGA this is usually the wrong thing to build.** It is here
//! because sometimes you genuinely need polar samples at the full
//! sample rate in hardware — a hardware AGC loop, a real-time phase
//! detector — and when you do, there is no cheaper correct option. But
//! that situation is rarer than it looks, and the costs are large:
//!
//! - **A pipelined CORDIC is one stage per bit of accuracy.** Measured
//!   from the emitted Verilog for the default configuration at sixteen
//!   iterations:
//!
//!   | | |
//!   |---|---|
//!   | adders / subtractors | **102** |
//!   | register declarations | **613** |
//!   | multipliers | 1 (the gain correction) |
//!   | latency | **16 cycles** |
//!
//!   That is a lot of fabric for one arithmetic operation. For
//!   comparison, the entire quadrature oscillator
//!   ([`crate::dsp::nco`]) uses two block RAMs, two multipliers and one
//!   cycle.
//!
//!   These went up slightly — from 101 and 553 — when the stage chain
//!   became a loop so the iteration count could be a parameter. The
//!   loop emits the same structure (no address mux, no ROM: the index
//!   folds to a constant during lowering) plus the constants bundle,
//!   which folds away in synthesis. Register *declarations* here counts
//!   `reg`s in the emitted module, most of them combinational
//!   temporaries, not flip-flops; the flip-flops are the sixteen
//!   pipeline stages. `report_the_resource_cost` prints these numbers, so they
//!   stay honest if the implementation changes.
//! - **The gain must be compensated.** The algorithm scales its input
//!   by `K = 1.6468…`, so either the input is pre-scaled or the output
//!   is corrected — one more multiply, or a further loss of accuracy if
//!   folded into a shift.
//! - **Most DSP chains never need polar at all.** Filtering, mixing,
//!   decimation and AGC all work in rectangular. Detection thresholds
//!   can compare `x² + y²` against a squared threshold and skip the
//!   square root entirely.
//! - **If magnitude alone is wanted, cheaper approximations exist.**
//!   Alpha-max-plus-beta-min gets within a few percent using one
//!   comparison and two shifts, which is a fraction of a CORDIC.
//!
//! **In an NMR or MRI receiver specifically, the usual answer is to
//! decimate first and convert in software.** After the DDC the sample
//! rate is orders of magnitude lower, the host has floating point, and
//! `atan2` there is exact rather than 16-iteration-approximate. Putting
//! this in the FPGA at 125 MHz spends logic to compute something nobody
//! reads until after decimation.
//!
//! Use it when the answer is needed *in hardware, at rate, in a
//! feedback path*. Otherwise decimate and let software do it.
//!
//! # What it does provide
//!
//! Both directions, pipelined at one sample per clock:
//!
//! - [`vectoring`] — `Iq` → magnitude and phase. Also computes `atan2`
//!   over the full circle.
//! - [`rotation`] — magnitude and phase → `Iq`.
//!
//! The two are exact inverses to within their quantisation, which
//! `rotation`'s round-trip test asserts directly.

use rhdl::prelude::*;

pub mod rotation;
pub mod vectoring;

pub use rotation::CordicRotation;
pub use vectoring::CordicVectoring;

/// Iterations, and therefore pipeline stages and cycles of latency.
pub const ITERATIONS: usize = 16;

/// Internal datapath width for the validated default configuration.
///
/// Wider than the sample width because the algorithm grows its operands
/// by the gain `K = 1.6468…` and needs headroom for the intermediate
/// rotations. Both widgets are generic over this now; the constant is
/// the default the aliases below use.
pub const INT_W: usize = 22;

/// The headroom the internal datapath needs above the sample width.
///
/// Two bits for the CORDIC gain `K = 1.6468…` (which is under 2), one
/// for the `sqrt(2)` a magnitude can reach when both components are at
/// full scale, and one for the sign. Checked at construction rather
/// than left to the caller to remember — a too-narrow datapath does not
/// fail loudly, it silently clips the largest vectors.
pub const INT_HEADROOM: usize = 4;

/// Is `int_w` wide enough to carry a `w`-bit sample through the
/// rotations without clipping?
pub const fn int_width_is_sufficient(w: usize, int_w: usize) -> bool {
    int_w >= w + INT_HEADROOM
}

/// Angle width for the validated default configuration. A full turn is
/// `2^ANGLE_W`.
///
/// **This does not match `dsp::nco`'s phase.** An earlier version of
/// this comment claimed it did — "so an angle from here can be fed to
/// the oscillator without rescaling" — and that was wrong. The `18` in
/// the NCO is [`crate::dsp::nco::sin_cos_linear_interp::AMP_W`], the
/// *amplitude* width. Its phase is
/// [`crate::dsp::nco::config::PHASE_W`] = 48 in the accumulator and
/// `TOTAL_W` = 22 after truncation, so feeding an angle from here to
/// the oscillator needs a left shift of 30 or 4 bits respectively.
///
/// The widgets are generic over this now; the constant is only the
/// default the aliases use.
pub const ANGLE_W: usize = 18;

/// `atan(2^-i)` for each iteration at [`ANGLE_W`], in turn units where a
/// full turn is `2^ANGLE_W`.
///
/// Retained for the default configuration and as the reference the
/// generic builder is checked against; [`atan_table`] computes the same
/// values for any angle width.
pub const ATAN_TABLE: [i128; ITERATIONS] = [
    32768, 19344, 10221, 5188, 2604, 1303, 652, 326, 163, 81, 41, 20, 10, 5, 3, 1,
];

/// How many iterations are worth running at a given angle width.
///
/// Beyond a point the arctangent rounds to zero and the stage does
/// nothing but add latency and area. In turn units the entry for
/// iteration `i` is `atan(2^-i)·2^angle_w/2π`, which stays at or above
/// a half — and so rounds to at least one — while
/// `2^(angle_w - i) >= π`, i.e. while `i <= angle_w - 2`.
///
/// So the count is **determined by the angle width, not chosen**. At
/// `angle_w = 18` this gives 16, the value the default configuration
/// has always used.
///
/// It is a separate const generic on the widgets rather than derived in
/// the type, because computing an array length from another const
/// generic needs `generic_const_exprs`. `Default` asserts the two
/// agree — the same pattern [`crate::dsp::nco::composite::Nco`] uses
/// for its truncation width.
pub const fn iterations_for(angle_w: usize) -> usize {
    angle_w - 2
}

/// The width-dependent constants a CORDIC stage needs.
///
/// Bundled into one [`crate::core::constant::Constant`] rather than
/// written as literals in the kernel, because with a generic angle
/// width they are no longer knowable at the source level. A `Constant`
/// folds away in synthesis, so this costs nothing in hardware.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct CordicConsts<const ANGLE_W: usize, const N: usize>
where
    rhdl::bits::W<ANGLE_W>: BitWidth,
{
    /// `atan(2^-i)` per iteration, in turn units.
    pub atan: [SignedBits<ANGLE_W>; N],
    /// Half a turn: the most negative representable angle, which is the
    /// same point on the circle as `+2^(ANGLE_W-1)` and, unlike it, fits.
    pub half_turn: SignedBits<ANGLE_W>,
    /// `1/K` in Q17, where `K` is the CORDIC gain for **this** `N`.
    ///
    /// Depends on the iteration count, not the angle width: `K` is the
    /// product of `sqrt(1 + 2^-2i)` over the iterations actually run.
    /// Twenty bits holds the Q17 value with room for the sign.
    pub inv_gain_q17: SignedBits<20>,
}

impl<const ANGLE_W: usize, const N: usize> Default for CordicConsts<ANGLE_W, N>
where
    rhdl::bits::W<ANGLE_W>: BitWidth,
{
    fn default() -> Self {
        Self::build()
    }
}

impl<const ANGLE_W: usize, const N: usize> CordicConsts<ANGLE_W, N>
where
    rhdl::bits::W<ANGLE_W>: BitWidth,
{
    /// Compute the constants for this configuration.
    ///
    /// Ordinary floating point at construction time rather than a
    /// `const fn`: the values need `atan` and `sqrt`, and a widget is
    /// built on the host where those are available. Nothing here
    /// reaches the kernel except the finished integers.
    pub fn build() -> Self {
        let turn = (1u128 << ANGLE_W) as f64;
        let mut atan = [SignedBits::<ANGLE_W>::default(); N];
        for (i, slot) in atan.iter_mut().enumerate() {
            let v = (2.0f64).powi(-(i as i32)).atan() * turn / std::f64::consts::TAU;
            *slot = signed::<ANGLE_W>(v.round() as i128);
        }
        // K = prod sqrt(1 + 2^-2i) over the iterations actually run.
        let mut gain = 1.0f64;
        for i in 0..N {
            gain *= (1.0 + (2.0f64).powi(-2 * (i as i32))).sqrt();
        }
        let inv_gain = ((1.0 / gain) * (1u128 << 17) as f64).round() as i128;
        Self {
            atan,
            half_turn: signed::<ANGLE_W>(-(1i128 << (ANGLE_W - 1))),
            inv_gain_q17: signed::<20>(inv_gain),
        }
    }
}

/// Half a turn, as a signed angle.
///
/// The most negative representable value at [`ANGLE_W`] bits — which is
/// the same point on the circle as `+2^(ANGLE_W-1)`, and unlike it, is
/// representable.
///
/// Written as a literal because `-(1 << (ANGLE_W - 1))` applies the
/// negation to an *unsigned* shift result before `signed()` converts
/// it, which the kernel compiler rejects.
pub const HALF_TURN: i128 = -131_072;

/// `1/K` in Q17, where `K = 1.6467602581…` is the CORDIC gain.
///
/// Applied as a multiply rather than folded into shifts: the shift
/// approximation costs accuracy the sixteen iterations were spent
/// buying.
pub const INV_GAIN_Q17: i128 = 79_594;

/// Re-exported: this lives in [`crate::dsp`] now, having been needed by
/// a fourth widget (`dsp::cic`).
pub use crate::dsp::sign_extend;
