#![warn(missing_docs)]
//! Finite impulse response filters.
//!
//! Two implementations of one interface. Both take [`In`] and produce
//! [`Out`], so they are interchangeable in any slot:
//!
//! - [`Fir`] — arbitrary taps, one multiplier per tap. The general
//!   case: any impulse response, symmetric or not, any length.
//! - [`SymmetricFir`] — odd-length symmetric taps only, folded so
//!   pairs sharing a coefficient are added before the multiply. Half
//!   the multipliers and exactly linear phase, at the cost of only
//!   accepting tap sets that have those properties.
//!
//! Prefer `SymmetricFir` when the taps qualify — a CIC compensator's
//! do — and `Fir` when they do not.
//!
//! This module is the hardware that *executes* a tap set, not the thing
//! that chooses one. See [`crate::dsp::cic::compensator`] for the
//! design side.
//!
//! # Both need `rhdl::prelude` in scope
//!
//! [`In`] and [`Out`] are `Digital` structs; nothing unusual, but they
//! are declared here rather than per-widget so that a filter can be
//! swapped without touching the code around it.

use rhdl::prelude::*;

pub mod general;
pub mod symmetric;

pub use general::Fir;

pub use symmetric::SymmetricFir;

/// Inputs to any FIR in this module.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W_IN: usize>
where
    rhdl::bits::W<W_IN>: BitWidth,
{
    /// The input sample, or `None` for an idle cycle.
    ///
    /// An idle cycle holds the delay line. A FIR's state is a window
    /// over *samples*, not over cycles, so a gap in the stream must not
    /// be read as a zero — the same rule the CIC follows, and what
    /// makes these filters correct on the CIC's one-in-`R` cadence.
    pub sample: Option<SignedBits<W_IN>>,
    /// Downstream's ready, per the `RCStream` contract.
    pub downstream_ready: bool,
}

/// Outputs from any FIR in this module.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W_OUT: usize>
where
    rhdl::bits::W<W_OUT>: BitWidth,
{
    /// The filtered sample, one per input sample.
    pub sample: Option<SignedBits<W_OUT>>,
    /// The result did not fit `W_OUT` and was clamped.
    ///
    /// Not a warning to be ignored: a compensator with gain above one
    /// can legitimately produce this on near-full-scale input, and it
    /// means the headroom budget is wrong somewhere upstream.
    pub saturated: bool,
    /// A sample was produced while `downstream_ready` was low.
    pub overrun: bool,
}

// Width arithmetic lives in `rhdl-dsp-design` for the same reason the
// CIC's does: a proc macro has to be able to compute it, and the macro
// layer may not depend on `rhdl-core`.
pub use rhdl_dsp_design::fir::{accumulator_width, accumulator_width_is_sufficient};
