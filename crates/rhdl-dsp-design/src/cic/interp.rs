#![warn(missing_docs)]
//! Register growth for a CIC *interpolator*, and why Hogenauer's
//! pruning schedule does not transfer to one.
//!
//! An interpolator is the decimator run backwards:
//!
//! ```text
//!   x[m] --> [ comb ]^N --> (upsample by R) --> [ integrate ]^N --> y[n]
//!            at the input rate                  at the output rate
//! ```
//!
//! Same `sinc^N` response, same `(R·M)`-spaced nulls, same absence of
//! multipliers. The two structural facts that follow from reversing the
//! order are what this module is about, and they are not symmetric with
//! the decimator's.
//!
//! # Fact one: the widths taper *losslessly*, by growth
//!
//! Every stage of a decimator must be at least
//! [`super::accumulator_width`] bits wide, because its integrators wrap
//! and only the comb section undoes the wrap. An interpolator has no
//! comb section after its integrators, so the reasoning has to be
//! redone — and it comes out better.
//!
//! Each stage's output is the response of a *finite* filter applied to
//! a bounded input, so each stage has its own exact bound:
//!
//! ```text
//!   G_j = 2^j                            j = 1..N     (combs, low rate)
//!   G_j = 2^(2N-j) · (R·M)^(j-N) / R     j = N+1..2N  (integrators, high rate)
//! ```
//!
//! The `1/R` on the integrator stages is the zero-stuffing: only one
//! input in `R` is nonzero, so the running sums see a signal diluted by
//! `R` relative to what the transfer function alone would suggest.
//! [`stage_gain_ratio`] returns `G_j` as an exact `numerator/R` pair
//! rather than a float, so every width here is integer arithmetic.
//!
//! Size stage `j` to `w_in + ceil(log2 G_j)` and it holds its own value
//! exactly: no truncation, no wrap, **no error at all**. That is the
//! important difference from the decimator, where tapering trades
//! noise for area. Here it is free, and
//! [`stage_width`] is the schedule.
//!
//! The saving is real. At `w_in = 18, N = 3, R = 125, M = 1` a uniform
//! filter spends 32 bits in all six stages; tapered they are
//! `19, 20, 21, 20, 26, 32`.
//!
//! Note the dip at the fourth stage. The taper is **not** monotonic
//! across the comb-to-integrator boundary: the last comb needs 21 bits
//! and the first integrator only 20, because the zero-stuffing divides
//! the signal by `R` faster than one integrator re-grows it by `R·M`.
//! [`gain_bits`] therefore maximises over every stage rather than
//! reading `G_2N`, and at `R·M = 2` the widest stage is in the comb
//! section outright.
//!
//! # Fact two: an interpolator cannot be pruned the way a decimator can
//!
//! [`super::prune`] implements Hogenauer's §V schedule, which lets a
//! decimator stage discard low-order bits because the truncation noise
//! is shaped by the *remaining* filter, and what remains is FIR.
//!
//! Reverse the order and that stops being true. The response from any
//! point inside an interpolator to its output contains the integrators
//! that follow, and an integrator has a pole at DC. Truncation has a
//! systematic `-1/2` LSB bias, and integrating a bias `k` times grows
//! it as `n^k` — without bound, forever. Concretely, the response after
//! comb stage `j` is
//!
//! ```text
//!   (1 - z^-RM)^(N-j) / (1 - z^-1)^N
//! ```
//!
//! which has `N` poles and only `N-j` orders of zero to cancel them.
//! `sum_k h_j(k)^2` diverges, so the error gain Hogenauer's rule
//! divides by does not exist. The same applies between integrators: the
//! response after integrator `j` is `1/(1-z^-1)^(2N-j)`, finite only
//! for the last one.
//!
//! **So the only place an interpolator may truncate is after its final
//! integrator**, where truncation is ordinary output quantisation. This
//! is not a limitation of the analysis; it is a property of the
//! structure. Fact one is what makes it affordable: the taper that
//! costs a decimator noise costs an interpolator nothing, so there is
//! less to want from pruning in the first place.
//!
//! # A variable rate composes with all of this
//!
//! [`crate::cic::prune`]'s schedule depends on `R` through the error
//! gain, so a run-time-variable `R` would need a schedule valid across
//! the whole range — the minimum permitted discard at every stage,
//! which is a real design compromise.
//!
//! Growth-based tapering has no such problem. `G_j` is monotonic in
//! `R`, so sizing every stage for `R_MAX` is exact for `R_MAX` and
//! merely generous for anything smaller. A smaller rate produces a
//! smaller value in a register that already held a larger one. Nothing
//! degrades, and there is nothing to choose.
//!
//! # The droop is nearly rate-independent, which is why one
//! compensator serves the whole range
//!
//! Measured against the *low* rate — the rate a compensator runs at on
//! either side of a CIC — the droop is
//!
//! ```text
//!   |sin(pi·u·M) / (R·M·sin(pi·u/R))|^N  ->  |sinc(u·M)|^N   as R grows
//! ```
//!
//! so [`super::response::magnitude_out`] barely moves with `R` above
//! about eight. One tap set designed at `R_MAX` therefore compensates
//! the whole range to within a small fraction of a dB, which is what
//! makes a variable-rate interpolator compensable at all.
//! `one_compensator_serves_the_whole_rate_range` measures it rather
//! than asserting it.

use super::ceil_log2;

/// Smallest `b` with `2^b · den >= num`, for `num, den >= 1`.
///
/// `ceil(log2(num/den))` without leaving the integers, which is what
/// keeps every width in this module exact and `const`.
pub const fn ceil_log2_ratio(num: u128, den: u128) -> usize {
    let mut b = 0;
    // `num` and `den` are both at least one, so this terminates as soon
    // as the shift exceeds the ratio.
    while (den << b) < num {
        b += 1;
    }
    b
}

/// `(R·M)^k`, saturating rather than wrapping.
///
/// Saturation is the safe direction: an overstated gain makes a stage
/// *wider*, which costs area rather than correctness.
const fn rm_pow(r: usize, m: usize, k: usize) -> u128 {
    let rm = (r * m) as u128;
    let mut acc: u128 = 1;
    let mut i = 0;
    while i < k {
        acc = acc.saturating_mul(rm);
        i += 1;
    }
    acc
}

/// Stage `j`'s exact gain bound, as `(numerator, denominator)`.
///
/// One-based, and ordered the way the signal travels: `j = 1..N` are
/// the combs at the input rate, `j = N+1..2N` the integrators at the
/// output rate. Returned as a ratio because the integrator stages'
/// bound carries the zero-stuffing's `1/R` and is generally not an
/// integer — see the module docs.
pub const fn stage_gain_ratio(j: usize, n: usize, r: usize, m: usize) -> (u128, u128) {
    if j <= n {
        // Comb stage: (1 - z^-M)^j, whose coefficients are the signed
        // binomials of order j and therefore sum to 2^j in absolute
        // value.
        (1u128 << j, 1)
    } else {
        // Integrator stage: 2^(2N-j) · (R·M)^(j-N) / R.
        let k = j - n;
        let num = rm_pow(r, m, k).saturating_mul(1u128 << (2 * n - j));
        (num, r as u128)
    }
}

/// Bits of growth stage `j` needs above the input width.
pub const fn stage_gain_bits(j: usize, n: usize, r: usize, m: usize) -> usize {
    let (num, den) = stage_gain_ratio(j, n, r, m);
    ceil_log2_ratio(num, den)
}

/// Width of stage `j` (one-based), tapered to its own growth.
///
/// Lossless: the stage holds its own value exactly, so a tapered
/// interpolator is *bit-identical* to a uniform-width one. That is a
/// much stronger property than the decimator's schedule offers, and
/// `crates/rhdl-fpga` asserts it rather than taking it on trust.
pub const fn stage_width(j: usize, w_in: usize, n: usize, r: usize, m: usize) -> usize {
    w_in + stage_gain_bits(j, n, r, m)
}

/// Bits of growth the *widest* stage needs.
///
/// The maximum over every stage, not `G_2N`, because the two sections
/// do not order the way intuition suggests: with `M = 1` the last comb
/// needs `N` bits of growth and the first integrator only `N-1`, the
/// zero-stuffing having diluted the signal faster than one integrator
/// re-grows it. At `R·M = 2` the comb section is the widest part of the
/// whole filter.
pub const fn gain_bits(n: usize, r: usize, m: usize) -> usize {
    let mut worst = 0;
    let mut j = 1;
    while j <= 2 * n {
        let b = stage_gain_bits(j, n, r, m);
        if b > worst {
            worst = b;
        }
        j += 1;
    }
    worst
}

/// The uniform width every stage of an unstapered interpolator needs.
///
/// `w_in +` [`gain_bits`]. Use [`stage_width`] instead when the stages
/// may differ; this is the width for the simple case where they may
/// not.
pub const fn accumulator_width(w_in: usize, n: usize, r: usize, m: usize) -> usize {
    w_in + gain_bits(n, r, m)
}

/// Is `w_acc` wide enough for this configuration?
///
/// Unlike the decimator's [`super::accumulator_width_is_sufficient`],
/// a `false` here means the *output* is wrong rather than the
/// intermediate state: an interpolator's last integrator is its output,
/// so a wrap there is a wrap in the answer.
pub const fn accumulator_width_is_sufficient(
    w_in: usize,
    w_acc: usize,
    n: usize,
    r: usize,
    m: usize,
) -> bool {
    w_acc >= accumulator_width(w_in, n, r, m)
}

/// The exact signal gain from input to output, as `(numerator, R)`.
///
/// `(R·M)^N / R`. Not `(R·M)^N`: the transfer function's DC gain is
/// that, but the zero-stuffing divides the signal by `R` on the way in,
/// and the gain a caller has to undo is the one the *signal* sees.
///
/// Exposed as a ratio, and deliberately not applied — as with the
/// decimator, where to rescale depends on what comes next.
pub const fn dc_gain_ratio(n: usize, r: usize, m: usize) -> (u128, u128) {
    (rm_pow(r, m, n), r as u128)
}

/// Width needed for a phase counter that counts to `r`.
///
/// Re-exported shape of [`super::counter_width`]; an interpolator's
/// counter runs at the *output* rate and gates the input instead of the
/// output, but it counts the same number of states.
pub const fn counter_width(r: usize) -> usize {
    super::counter_width(r)
}

/// Width needed to *carry* a rate up to `r`, as a value.
///
/// **One bit more than [`counter_width`] when `r` is a power of two**,
/// and that difference is a real bug waiting to happen. A counter needs
/// to represent `0..r-1`; a variable-rate input carries `r` itself, so
/// `r = 8` needs four bits where counting to eight needs three.
///
/// The widget compares `phase + 1 >= rate`, so both live in the same
/// register width and this is the one to size it by. The alternative —
/// carrying `r - 1` and saving the bit — moves the off-by-one into
/// every caller, which is the wrong place for it.
pub const fn rate_width(r: usize) -> usize {
    let b = ceil_log2(r + 1);
    if b == 0 { 1 } else { b }
}

/// Bits of state a uniform-width interpolator spends, for comparison.
///
/// `2N` stages at [`accumulator_width`], plus `M-1` extra registers per
/// comb stage for the differential delay line.
pub const fn uniform_state_bits(w_in: usize, n: usize, r: usize, m: usize) -> usize {
    let w = accumulator_width(w_in, n, r, m);
    // N integrators, plus N comb stages each holding an M-deep line.
    n * w + n * m * w
}

/// Bits of state the tapered schedule spends.
pub const fn tapered_state_bits(w_in: usize, n: usize, r: usize, m: usize) -> usize {
    let mut total = 0;
    let mut j = 1;
    while j <= 2 * n {
        let w = stage_width(j, w_in, n, r, m);
        // Comb stages carry an M-deep delay line; integrators carry one
        // register.
        total += if j <= n { m * w } else { w };
        j += 1;
    }
    total
}

/// Growth bits for the widest stage, ignoring the zero-stuffing.
///
/// The number a reader gets by applying the *decimator's* rule to an
/// interpolator, kept here so the difference is measurable rather than
/// merely described: it is [`super::gain_bits`], and it overstates the
/// requirement by `ceil(log2 R)` bits at the output stage.
pub const fn naive_gain_bits(n: usize, r: usize, m: usize) -> usize {
    super::gain_bits(n, r, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cic::response;

    /// The comb section's bound is the binomial absolute sum.
    #[test]
    fn comb_stages_grow_one_bit_each() {
        for n in 1..=5 {
            for j in 1..=n {
                assert_eq!(
                    stage_gain_bits(j, n, 64, 1),
                    j,
                    "comb stage {j} of {n} grows {j} bits"
                );
            }
        }
    }

    /// The output stage's bound is the signal gain, which is where the
    /// zero-stuffing shows up.
    #[test]
    fn the_output_stage_matches_the_signal_gain() {
        for &(n, r, m) in &[(1, 8, 1), (2, 8, 1), (3, 125, 1), (4, 25, 1), (2, 8, 2)] {
            let (gn, gd) = dc_gain_ratio(n, r, m);
            let (sn, sd) = stage_gain_ratio(2 * n, n, r, m);
            assert_eq!((gn, gd), (sn, sd), "N={n} R={r} M={m}");
        }
    }

    /// `(R·M)^N / R`, checked by hand on the configuration the DUC
    /// this was written for uses.
    #[test]
    fn the_gain_is_r_to_the_n_minus_one_at_unit_delay() {
        // N = 3, R = 125, M = 1 -> 125^3/125 = 15625 = 125^2.
        assert_eq!(dc_gain_ratio(3, 125, 1), (125 * 125 * 125, 125));
        assert_eq!(gain_bits(3, 125, 1), 14, "ceil(log2 15625) = 14");
        assert_eq!(accumulator_width(18, 3, 125, 1), 32);
    }

    /// **The widest stage is not always the last one.**
    ///
    /// With `M = 1` the zero-stuffing dilutes faster than one
    /// integrator re-grows, so the last comb is wider than the first
    /// integrator; at `R·M = 2` the comb section is the widest part of
    /// the filter. `gain_bits` takes the maximum over all stages for
    /// exactly this reason, and a version that assumed `j = 2N` would
    /// under-size here.
    #[test]
    fn the_comb_section_can_be_the_widest_part() {
        let (n, r, m) = (3, 2, 1);
        // Combs: 1, 2, 3 bits. Integrators: 2^(2N-j)·2^(j-N)/2 =
        // 2^(N-1) = 4, flat -> 2 bits each.
        assert_eq!(stage_gain_bits(3, n, r, m), 3, "last comb");
        assert_eq!(stage_gain_bits(4, n, r, m), 2, "first integrator");
        assert_eq!(stage_gain_bits(6, n, r, m), 2, "last integrator");
        assert_eq!(gain_bits(n, r, m), 3, "the maximum is in the comb section");
        // And the naive rule would have said N·log2(RM) = 3.
        assert_eq!(naive_gain_bits(n, r, m), 3);
    }

    /// The taper is monotonic through the integrators, which is what
    /// lets a hardware generator emit widening stages in order.
    #[test]
    fn the_integrator_taper_never_narrows() {
        for &(n, r, m) in &[(2, 8, 1), (3, 125, 1), (4, 25, 1), (3, 16, 2), (5, 1024, 1)] {
            for j in (n + 1)..(2 * n) {
                let a = stage_gain_bits(j, n, r, m);
                let b = stage_gain_bits(j + 1, n, r, m);
                assert!(
                    b >= a,
                    "N={n} R={r} M={m}: stage {j} needs {a} bits, {} needs {b}",
                    j + 1
                );
            }
        }
    }

    /// No stage exceeds the uniform width, so a tapered filter always
    /// fits where a uniform one did.
    #[test]
    fn no_stage_is_wider_than_the_uniform_width() {
        for &(n, r, m) in &[(1, 2, 1), (2, 8, 1), (3, 125, 1), (4, 25, 1), (5, 1024, 1)] {
            let uniform = accumulator_width(18, n, r, m);
            for j in 1..=(2 * n) {
                assert!(stage_width(j, 18, n, r, m) <= uniform, "N={n} R={r} j={j}");
            }
        }
    }

    /// The taper is worth doing, with the numbers quoted in the module
    /// docs rather than a vague claim.
    #[test]
    fn the_taper_is_a_real_saving() {
        let (w_in, n, r, m) = (18, 3, 125, 1);
        let widths: Vec<usize> = (1..=(2 * n))
            .map(|j| stage_width(j, w_in, n, r, m))
            .collect();
        // Combs +1,+2,+3; then 4·125/125 = 4 -> +2, 2·125 = 250 -> +8,
        // 15625 -> +14. The dip from 21 to 20 at the boundary is the
        // zero-stuffing and is the module docs' worked example.
        assert_eq!(widths, vec![19, 20, 21, 20, 26, 32]);
        assert!(
            tapered_state_bits(w_in, n, r, m) < uniform_state_bits(w_in, n, r, m),
            "tapered {} vs uniform {}",
            tapered_state_bits(w_in, n, r, m),
            uniform_state_bits(w_in, n, r, m)
        );
    }

    /// **Sizing for `R_MAX` covers every smaller rate.**
    ///
    /// The property a run-time-variable rate rests on. Not obvious
    /// enough to assert: `G_j` for the integrator stages is
    /// `2^(2N-j)(RM)^(j-N)/R`, whose `R` dependence is
    /// `R^(j-N-1)` — flat at `j = N+1` and increasing after, so
    /// monotonic but only weakly at the first integrator.
    #[test]
    fn sizing_for_the_maximum_rate_covers_every_smaller_one() {
        for &(n, m) in &[(1, 1), (2, 1), (3, 1), (4, 1), (3, 2)] {
            for r_max in [4usize, 16, 125, 1000] {
                let bound = gain_bits(n, r_max, m);
                for r in 2..=r_max {
                    assert!(
                        gain_bits(n, r, m) <= bound,
                        "N={n} M={m}: R={r} needs {} bits, R_MAX={r_max} allows {bound}",
                        gain_bits(n, r, m)
                    );
                }
            }
        }
    }

    /// **One tap set serves the whole rate range**, measured.
    ///
    /// The claim that makes a variable-rate interpolator compensable:
    /// the droop, expressed against the low rate, is almost independent
    /// of `R`. Compare the response a compensator designed at `R_MAX`
    /// would invert against the response actually presented at each
    /// smaller rate, across the passband.
    #[test]
    fn one_compensator_serves_the_whole_rate_range() {
        let (n, m) = (3usize, 1usize);
        let r_max = 125usize;
        let mut worst_db = 0.0f64;
        let mut worst_at = (0usize, 0.0f64);
        for r in [8usize, 16, 32, 64, 125] {
            // Out to 40% of the low-rate Nyquist, which is more
            // passband than a DUC envelope normally occupies.
            for k in 0..=40 {
                let u = 0.4 * 0.5 * (k as f64) / 40.0;
                let a = response::magnitude_out(u, n, r, m);
                let b = response::magnitude_out(u, n, r_max, m);
                let d = 20.0 * (a / b).log10();
                if d.abs() > worst_db.abs() {
                    worst_db = d;
                    worst_at = (r, u);
                }
            }
        }
        // Measured, and the number is in the module docs. If this moves,
        // the docs are wrong.
        assert!(
            worst_db.abs() < 0.05,
            "worst mismatch {worst_db:.4} dB at R={} u={:.4}",
            worst_at.0,
            worst_at.1
        );
    }

    /// And the same claim fails, as it should, for a rate small enough
    /// that `sin(pi u / R)` has not yet become its argument.
    ///
    /// A test that only confirms is not evidence the quantity was
    /// measured; this one establishes that the tolerance above is
    /// tight enough to detect a violation.
    #[test]
    fn the_rate_independence_does_break_at_small_r() {
        let (n, m) = (3usize, 1usize);
        let u = 0.2;
        let small = response::magnitude_out(u, n, 2, m);
        let large = response::magnitude_out(u, n, 125, m);
        let d = 20.0 * (small / large).log10();
        // 0.43 dB, against the 0.027 dB the same measurement gives at
        // R = 8. So the 0.05 dB tolerance above is roughly an order of
        // magnitude below a real violation, which is what makes it
        // evidence rather than decoration.
        assert!(
            d.abs() > 0.3,
            "R=2 should differ audibly from R=125, got {d:.4} dB"
        );
        let at_eight = {
            let a = response::magnitude_out(u, n, 8, m);
            20.0 * (a / large).log10()
        };
        assert!(
            d.abs() > 10.0 * at_eight.abs(),
            "R=2 ({d:.4} dB) should be far worse than R=8 ({at_eight:.4} dB)"
        );
    }

    /// **A rate needs one more bit than a counter, at the powers of
    /// two.**
    ///
    /// The distinction that produced a panic the first time this was
    /// written: `bits::<3>(8)` does not exist, so a widget carrying
    /// `R = 8` in a three-bit field fails at construction rather than
    /// silently truncating. Pinned so the two functions cannot drift
    /// back together.
    #[test]
    fn a_rate_needs_one_more_bit_than_a_counter_at_the_powers_of_two() {
        for r in [2usize, 4, 8, 16, 64, 1024] {
            assert_eq!(
                rate_width(r),
                counter_width(r) + 1,
                "R={r} is a power of two"
            );
        }
        for r in [3usize, 5, 7, 9, 125, 1000] {
            assert_eq!(rate_width(r), counter_width(r), "R={r} is not");
        }
        // And every rate up to the maximum fits the field.
        for r in 2usize..=200 {
            assert!(r < (1usize << rate_width(r)), "R={r}");
        }
    }

    /// `ceil_log2_ratio` agrees with `ceil_log2` when the denominator
    /// is one, and is exact at the powers of two either side.
    #[test]
    fn the_ratio_logarithm_is_exact_at_the_boundaries() {
        for v in 1u128..200 {
            assert_eq!(ceil_log2_ratio(v, 1), ceil_log2(v as usize));
        }
        assert_eq!(ceil_log2_ratio(8, 1), 3);
        assert_eq!(ceil_log2_ratio(9, 1), 4);
        assert_eq!(ceil_log2_ratio(16, 2), 3, "8");
        assert_eq!(ceil_log2_ratio(17, 2), 4, "8.5 rounds up");
        assert_eq!(ceil_log2_ratio(1, 125), 0, "a gain below one needs no bits");
    }

    /// The naive rule — the decimator's — overstates the output stage
    /// by **at least** `ceil(log2 R)` bits.
    ///
    /// Worth pinning, because applying the wrong one is the most likely
    /// mistake a reader familiar with the decimator will make, and it
    /// costs those bits in every stage of the filter.
    ///
    /// *At least*, not exactly: `naive` takes the ceiling per factor
    /// (`N·ceil(log2 R·M)`) while the exact bound takes one ceiling of
    /// the whole ratio, so the two differ by the accumulated rounding
    /// as well as by the zero-stuffing. `N = 4, R = 25` is the case
    /// that shows it — a gap of 6 where `ceil(log2 25) = 5`. An earlier
    /// version of this test asserted equality and was simply wrong.
    #[test]
    fn the_naive_rule_overstates_the_output_stage() {
        // The gaps, as data, so a change in either rule is visible.
        for &(n, r, m, gap) in &[
            (2, 8, 1, 3),
            (3, 125, 1, 7),
            (4, 25, 1, 6),
            (3, 64, 2, 6),
            (5, 1024, 1, 10),
        ] {
            let exact = stage_gain_bits(2 * n, n, r, m);
            let naive = naive_gain_bits(n, r, m);
            assert_eq!(naive - exact, gap, "N={n} R={r} M={m}");
            assert!(
                naive - exact >= ceil_log2(r),
                "N={n} R={r} M={m}: gap {} below ceil_log2(R) {}",
                naive - exact,
                ceil_log2(r)
            );
        }
    }
}
