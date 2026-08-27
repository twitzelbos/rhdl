#![warn(missing_docs)]
//! Cascaded integrator-comb decimation — the cheap way to drop sample rate.
//!
//! A CIC filter is a low-pass decimator built entirely from adders and
//! registers: **no multipliers and no coefficient storage**. That is
//! what makes it the standard front end of a digital down-converter,
//! where the sample rate coming off the converter is far higher than the
//! bandwidth of interest and the first job is to throw most of it away
//! without aliasing the signal.
//!
//! Structurally (Hogenauer, 1981):
//!
//! ```text
//!   x[n] --> [ integrate ]^N --> (decimate by R) --> [ comb ]^N --> y[m]
//!            at the input rate                       at the output rate
//! ```
//!
//! An integrator is `y[n] = y[n-1] + x[n]`; a comb is
//! `y[m] = x[m] - x[m-M]`. Cascading `N` of each gives a
//! `sinc^N`-shaped response whose nulls sit on the aliases the
//! decimation would otherwise fold in.
//!
//! # The two facts that make a CIC correct
//!
//! **The DC gain is `(R·M)^N`,** exactly, and it is not optional — the
//! integrators grow the signal and the combs do not shrink it. A CIC
//! that appears to "amplify by 4096" is working correctly; the scaling
//! belongs downstream where the factor is known.
//!
//! **The integrators overflow, and that is fine.** They are running
//! sums of a bounded input, so they wrap continuously. Hogenauer's
//! result is that two's-complement wrap is harmless *provided every
//! stage is at least [`accumulator_width`] bits wide*: the comb section
//! subtracts the wrapped values and the wraps cancel exactly. Narrow the
//! accumulator below that bound and the output is not merely noisy, it
//! is wrong — and wrong in a way that looks like a plausible signal.
//! [`accumulator_width`] is therefore checked at construction rather
//! than left to the caller.

pub mod cascaded;
pub mod compensated;
pub mod decimator;
pub mod interpolator;
pub mod pruned;
pub mod stream;

pub use decimator::CicDecimate;
pub use interpolator::CicInterpolate;

// The design mathematics lives in `rhdl-dsp-design`, a leaf crate with
// no RHDL dependency, because a proc macro must be able to reach it and
// `rhdl-macro-core` may not depend on `rhdl-core` (architecture.md §2).
// Re-exported here so callers -- and the `cic_pruned!` macro's
// `$crate::dsp::cic::prune::stage_width` paths -- see no difference.
pub use rhdl_dsp_design::cic::{
    accumulator_width, accumulator_width_is_sufficient, chain, compensator, counter_width, dc_gain,
    gain_bits, interp, prune, response,
};
