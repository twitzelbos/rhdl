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
//!   from the emitted Verilog for `CordicVectoring<18>` at sixteen
//!   iterations:
//!
//!   | | |
//!   |---|---|
//!   | adders / subtractors | **101** |
//!   | register declarations | **553** |
//!   | multipliers | 1 (the gain correction) |
//!   | latency | **16 cycles** |
//!
//!   That is a lot of fabric for one arithmetic operation. For
//!   comparison, the entire quadrature oscillator
//!   ([`crate::dsp::nco`]) uses two block RAMs, two multipliers and one
//!   cycle. `report_the_resource_cost` prints these numbers, so they
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

/// Internal datapath width. Wider than the sample width because the
/// algorithm grows its operands by the gain `K = 1.6468…` and needs
/// headroom for the intermediate rotations.
pub const INT_W: usize = 22;

/// Angle width. A full turn is `2^ANGLE_W`, matching the phase
/// convention used by [`crate::dsp::nco`], so an angle from here can be
/// fed to the oscillator without rescaling.
pub const ANGLE_W: usize = 18;

/// `atan(2^-i)` for each iteration, in turn units where a full turn is
/// `2^ANGLE_W`.
pub const ATAN_TABLE: [i128; ITERATIONS] = [
    32768, 19344, 10221, 5188, 2604, 1303, 652, 326, 163, 81, 41, 20, 10, 5, 3, 1,
];

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

/// Widen a signed value, sign-extending explicitly.
///
/// # Why this exists instead of `SignedBits::resize`
///
/// `resize` is the natural spelling and is **wrong for a value
/// unwrapped from an `Option`**: it emits zero extension there, while
/// the Rust simulator sign-extends. Tiers 1 and 2 therefore pass and
/// only the `iverilog` round-trip fails — or, as here, `descriptor()`
/// rejects a later negation with "cannot negate unsigned value",
/// because the widened value has lost its signedness.
///
/// The same operation on a *direct* input emits `$signed({{n{v[msb]}},
/// v})` correctly, so RHDL can do it; something about extraction from
/// an aggregate drops the signedness. This is the third place in the
/// tree to hit it (see also `dsp::nco::modulation` and
/// `tests/signed_literal_comparison.rs`), and it is filed as compiler
/// work.
///
/// Zero-extending and then correcting uses only bit operations and
/// addition, neither of which depends on the operand's declared
/// signedness, so it is correct either way.
#[kernel]
#[doc(hidden)]
pub fn sign_extend<const FROM: usize, const TO: usize>(v: SignedBits<FROM>) -> SignedBits<TO>
where
    rhdl::bits::W<FROM>: BitWidth,
    rhdl::bits::W<TO>: BitWidth,
{
    let raw = v.as_unsigned();
    let negative = (raw & bits::<FROM>(1 << (FROM - 1))) != bits::<FROM>(0);
    let widened = raw.resize::<TO>();
    let fill = if negative {
        bits::<TO>((1 << TO) - (1 << FROM))
    } else {
        bits::<TO>(0)
    };
    (widened + fill).as_signed()
}
