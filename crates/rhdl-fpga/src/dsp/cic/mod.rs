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

use rhdl::prelude::*;

pub mod compensated;
pub mod compensator;
pub mod decimator;
pub mod prune;
pub mod pruned;
pub mod response;

pub use decimator::CicDecimate;

/// Smallest `b` with `2^b >= v`, for `v >= 1`.
const fn ceil_log2(v: usize) -> usize {
    let mut b = 0;
    while (1usize << b) < v {
        b += 1;
    }
    b
}

/// Bits of growth the integrator cascade adds.
///
/// The DC gain is `(R·M)^N`, so the signal grows by `N·log2(R·M)` bits.
/// Computed as `N · ceil(log2(R·M))`, which is exact when `R·M` is a
/// power of two and conservative by at most `N` bits otherwise —
/// deliberately the safe direction, since too few bits corrupts the
/// output rather than degrading it.
pub const fn gain_bits(stages: usize, r: usize, m: usize) -> usize {
    stages * ceil_log2(r * m)
}

/// The accumulator width a CIC needs, per Hogenauer.
///
/// `w_in + N·log2(R·M)`. Every integrator and comb stage must be at
/// least this wide for the two's-complement wrap in the integrators to
/// cancel in the combs.
pub const fn accumulator_width(w_in: usize, stages: usize, r: usize, m: usize) -> usize {
    w_in + gain_bits(stages, r, m)
}

/// Is `w_acc` wide enough to carry this configuration without
/// corrupting the output?
pub const fn accumulator_width_is_sufficient(
    w_in: usize,
    w_acc: usize,
    stages: usize,
    r: usize,
    m: usize,
) -> bool {
    w_acc >= accumulator_width(w_in, stages, r, m)
}

/// Width needed for a decimation counter that counts to `r`.
pub const fn counter_width(r: usize) -> usize {
    let b = ceil_log2(r);
    if b == 0 { 1 } else { b }
}

/// The exact DC gain, `(R·M)^N`.
///
/// Exposed so a caller can undo it: the CIC deliberately does not, since
/// the right place to rescale depends on what comes next.
pub const fn dc_gain(stages: usize, r: usize, m: usize) -> u128 {
    let mut g: u128 = 1;
    let rm = (r * m) as u128;
    let mut i = 0;
    while i < stages {
        g *= rm;
        i += 1;
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_log2_is_right_at_the_boundaries() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(64), 6);
        assert_eq!(ceil_log2(65), 7);
    }

    /// The published bound, on the configuration Hogenauer's paper uses
    /// as its worked example.
    #[test]
    fn the_accumulator_bound_matches_hogenauer() {
        // N = 4, R = 25, M = 1, 16-bit input.  ceil(log2 25) = 5, so
        // 20 bits of growth.
        assert_eq!(gain_bits(4, 25, 1), 20);
        assert_eq!(accumulator_width(16, 4, 25, 1), 36);
        // Exact for a power-of-two rate: N=3, R=64 -> 18 bits.
        assert_eq!(gain_bits(3, 64, 1), 18);
        assert_eq!(accumulator_width(12, 3, 64, 1), 30);
    }

    /// The differential delay doubles the effective rate for growth
    /// purposes.
    #[test]
    fn the_differential_delay_counts_toward_growth() {
        assert_eq!(gain_bits(2, 8, 1), 6);
        assert_eq!(gain_bits(2, 8, 2), 8);
    }

    #[test]
    fn the_dc_gain_is_the_product() {
        assert_eq!(dc_gain(1, 8, 1), 8);
        assert_eq!(dc_gain(3, 4, 1), 64);
        assert_eq!(dc_gain(2, 8, 2), 256);
    }

    #[test]
    fn the_sufficiency_check_is_tight() {
        assert!(accumulator_width_is_sufficient(12, 30, 3, 64, 1));
        assert!(!accumulator_width_is_sufficient(12, 29, 3, 64, 1));
    }

    #[test]
    fn a_counter_always_has_at_least_one_bit() {
        assert_eq!(counter_width(1), 1);
        assert_eq!(counter_width(2), 1);
        assert_eq!(counter_width(64), 6);
        assert_eq!(counter_width(100), 7);
    }
}
