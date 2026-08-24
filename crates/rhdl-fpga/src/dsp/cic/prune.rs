#![warn(missing_docs)]
//! Hogenauer's register-pruning schedule, §V of the 1981 paper.
//!
//! The CIC *structure* is older than the paper; what Hogenauer
//! contributed — and what the title's "economical" refers to — is the
//! error analysis that lets each stage discard low-order bits without
//! degrading the output beyond its own quantisation. Later stages can
//! discard more, because their truncation noise is shaped by less of
//! the remaining filter, so the datapath **tapers**.
//!
//! Without it every stage runs at the worst-case width
//! [`super::accumulator_width`]. At `N = 5, R = 1024, B_in = 18` that
//! is 78 bits in ten registers and ten adders, most of them carrying
//! bits that cannot affect the output.
//!
//! # The analysis, and why it is exact integer arithmetic here
//!
//! Stages are numbered 1..N for the integrators, N+1..2N for the combs,
//! and 2N+1 for the final output truncation. Truncating `B_j` bits at
//! stage `j` injects noise that reaches the output amplified by the
//! **error gain**
//!
//! ```text
//!   F_j = sqrt( sum_k h_j(k)^2 )
//! ```
//!
//! where `h_j` is the impulse response of everything after stage `j`.
//! Hogenauer's rule spreads the budget evenly:
//!
//! ```text
//!   B_j = floor( -log2(F_j) + log2(sigma_T) + 0.5*log2(6/N) )
//! ```
//!
//! with `sigma_T = 2^B_out / sqrt(12)` the error the output truncation
//! is already allowed. Substituting and folding the constants:
//!
//! ```text
//!   B_j = floor( B_out - 0.5*log2( 2*N*S_j ) )      where S_j = sum_k h_j(k)^2
//!       = B_out - ceil_log4( 2*N*S_j )
//! ```
//!
//! **`S_j` is an integer**, so the whole schedule is exact integer
//! arithmetic and computable in a `const fn` — no floating point, no
//! rounding to argue about, and usable in a type position.
//!
//! # This is a design aid, not a guarantee
//!
//! The schedule says how many bits each stage *may* discard for a given
//! output budget. Whether the resulting noise is acceptable is a
//! question about the signal, and the honest check is behavioural:
//! run the pruned filter against a full-precision one and measure. That
//! is what `pruning_error_stays_within_the_budget` does.

/// `C(n, k)`, saturating rather than wrapping on overflow.
///
/// Saturation is the safe direction here: an overstated `S_j` makes the
/// schedule discard *fewer* bits, so a configuration large enough to
/// saturate ends up conservative rather than silently under-width.
const fn binom(n: u128, k: u128) -> u128 {
    if k > n {
        return 0;
    }
    let k = if k > n - k { n - k } else { k };
    let mut num: u128 = 1;
    let mut i: u128 = 0;
    while i < k {
        // num = num * (n - i) / (i + 1), staged to limit growth.
        let (m, over) = num.overflowing_mul(n - i);
        if over {
            return u128::MAX;
        }
        num = m / (i + 1);
        i += 1;
    }
    num
}

/// Smallest `t` with `4^t >= v`.
///
/// `floor(b - 0.5*log2(v))` equals `b - ceil_log4(v)`: the largest `x`
/// with `v <= 4^(b-x)`.
const fn ceil_log4(v: u128) -> usize {
    if v <= 1 {
        return 0;
    }
    let mut t = 0;
    let mut p: u128 = 1;
    while p < v {
        // p *= 4, saturating.
        if p > u128::MAX / 4 {
            return t + 1;
        }
        p *= 4;
        t += 1;
    }
    t
}

/// `sum_k h_j(k)^2` for stage `j`, one-based as in the paper.
///
/// Integrator stages (`j <= n`) see the whole comb section plus the
/// remaining integrators; comb stages see only the combs after them,
/// whose impulse response is a plain binomial alternating series.
#[allow(clippy::manual_is_multiple_of)]
pub const fn error_gain_squared(j: usize, n: usize, r: usize, m: usize) -> u128 {
    let rm = (r * m) as u128;
    let nn = n as u128;
    let jj = j as u128;
    let mut acc: u128 = 0;

    if j <= n {
        // k runs to (RM - 1)*N + j - 1.
        let kmax = (rm - 1) * nn + jj - 1;
        let mut k: u128 = 0;
        while k <= kmax {
            let lmax = k / rm;
            let mut h: i128 = 0;
            let mut l: u128 = 0;
            while l <= lmax {
                let a = binom(nn, l);
                let b = binom(k - rm * l + nn - jj, nn - jj);
                let term = a.saturating_mul(b);
                let term = if term > i128::MAX as u128 {
                    i128::MAX
                } else {
                    term as i128
                };
                h = if l % 2 == 0 {
                    h.saturating_add(term)
                } else {
                    h.saturating_sub(term)
                };
                l += 1;
            }
            let sq = h.saturating_mul(h);
            acc = acc.saturating_add(sq as u128);
            k += 1;
        }
    } else {
        // Comb stage: h_j(k) = (-1)^k * C(2N + 1 - j, k).
        let top = 2 * nn + 1 - jj;
        let mut k: u128 = 0;
        while k <= top {
            let c = binom(top, k);
            acc = acc.saturating_add(c.saturating_mul(c));
            k += 1;
        }
    }
    acc
}

/// Bits stage `j` may discard, given `b_out` bits discarded at the
/// output.
///
/// Clamped at zero: a stage whose error gain is large enough to make
/// the formula negative may discard nothing.
// `l % 2 == 0` rather than `is_multiple_of`: the latter is not stable
// in a `const fn` here.
#[allow(clippy::manual_is_multiple_of)]
pub const fn prune_bits(j: usize, n: usize, r: usize, m: usize, b_out: usize) -> usize {
    let s = error_gain_squared(j, n, r, m);
    let v = (2 * n as u128).saturating_mul(s);
    let t = ceil_log4(v);
    b_out.saturating_sub(t)
}

/// Width of integrator or comb stage `j` (one-based), after pruning.
///
/// The full width less what this stage may discard, floored at the
/// input width so a stage never becomes narrower than a sample.
pub const fn stage_width(
    j: usize,
    w_in: usize,
    n: usize,
    r: usize,
    m: usize,
    b_out: usize,
) -> usize {
    let full = super::accumulator_width(w_in, n, r, m);
    let cut = prune_bits(j, n, r, m, b_out);
    if full <= cut + w_in { w_in } else { full - cut }
}

#[cfg(test)]
mod tests {
    use super::super::accumulator_width;
    use super::*;

    #[test]
    fn binomials_are_right() {
        assert_eq!(binom(5, 0), 1);
        assert_eq!(binom(5, 1), 5);
        assert_eq!(binom(5, 2), 10);
        assert_eq!(binom(5, 5), 1);
        assert_eq!(binom(5, 6), 0);
        assert_eq!(binom(52, 5), 2_598_960);
    }

    #[test]
    fn ceil_log4_is_right_at_the_boundaries() {
        assert_eq!(ceil_log4(0), 0);
        assert_eq!(ceil_log4(1), 0);
        assert_eq!(ceil_log4(2), 1);
        assert_eq!(ceil_log4(4), 1);
        assert_eq!(ceil_log4(5), 2);
        assert_eq!(ceil_log4(16), 2);
        assert_eq!(ceil_log4(17), 3);
    }

    /// The last comb sees nothing after it but the output, so its
    /// impulse response is a single unit sample.
    #[test]
    fn the_final_comb_has_unit_error_gain() {
        let n = 4;
        // j = 2N is the last comb: 2N + 1 - j = 1, so h = [1, -1].
        assert_eq!(error_gain_squared(2 * n, n, 25, 1), 2);
    }

    /// **The taper goes the right way.**
    ///
    /// Later stages have smaller error gain, so they may discard more
    /// and end up narrower. A schedule that widened toward the output
    /// would be the analysis applied backwards — an easy sign error and
    /// one that would still produce a filter that mostly works.
    #[test]
    fn the_schedule_tapers_toward_the_output() {
        let (w_in, n, r, m, b_out) = (18, 4, 64, 1, 20);
        let widths: Vec<usize> = (1..=2 * n)
            .map(|j| stage_width(j, w_in, n, r, m, b_out))
            .collect();
        println!(
            "widths = {widths:?}  (full = {})",
            accumulator_width(w_in, n, r, m)
        );
        for w in widths.windows(2) {
            assert!(
                w[1] <= w[0],
                "the datapath must not widen toward the output: {widths:?}"
            );
        }
        assert!(
            widths[0] <= accumulator_width(w_in, n, r, m),
            "no stage may exceed the full width"
        );
        assert!(
            *widths.last().unwrap() < widths[0],
            "pruning that saves nothing is not pruning: {widths:?}"
        );
    }

    /// A larger output budget prunes harder.
    #[test]
    fn a_looser_output_budget_prunes_more() {
        let (w_in, n, r, m) = (18, 4, 64, 1);
        let tight: usize = (1..=2 * n).map(|j| stage_width(j, w_in, n, r, m, 8)).sum();
        let loose: usize = (1..=2 * n).map(|j| stage_width(j, w_in, n, r, m, 24)).sum();
        assert!(
            loose < tight,
            "discarding more at the output should let earlier stages \
             discard more too: {loose} vs {tight}"
        );
    }

    /// The saving is worth having at a realistic configuration.
    #[test]
    fn the_saving_is_substantial_at_a_deep_cascade() {
        let (w_in, n, r, m, b_out) = (18, 5, 1024, 1, 30);
        let full = accumulator_width(w_in, n, r, m);
        let pruned: usize = (1..=2 * n)
            .map(|j| stage_width(j, w_in, n, r, m, b_out))
            .sum();
        let unpruned = full * 2 * n;
        println!("full width {full}, unpruned total {unpruned}, pruned total {pruned}");
        assert!(
            pruned * 10 < unpruned * 8,
            "pruning should save at least a fifth of the register bits: \
             {pruned} vs {unpruned}"
        );
    }
}

#[cfg(test)]
mod headline_tests {
    use super::super::accumulator_width;
    use super::*;

    /// The numbers quoted in the `pruned` module docs, the example and
    /// the CHANGELOG.
    ///
    /// They are the argument for the whole macro existing, so they had
    /// better stay true. A schedule change that improves the taper is
    /// welcome; one that silently stops saving anything is not.
    #[test]
    fn the_quoted_saving_is_real() {
        const WI: usize = 18;
        const N: usize = 5;
        const R: usize = 1024;
        const M: usize = 1;
        const BO: usize = 30;
        let full = accumulator_width(WI, N, R, M);
        let widths: Vec<usize> = (1..=2 * N)
            .map(|j| stage_width(j, WI, N, R, M, BO))
            .collect();
        let tapered: usize = widths.iter().sum();
        assert_eq!(full, 68, "full accumulator width");
        assert_eq!(full * 2 * N, 680, "uniform total");
        assert_eq!(tapered, 517, "tapered total: {widths:?}");
        assert!(widths.windows(2).all(|w| w[0] >= w[1]));
    }

    /// The taper quoted in the `pruned` module's internals diagram.
    #[test]
    fn the_diagram_matches_the_schedule() {
        let widths: Vec<usize> = (1..=8).map(|j| stage_width(j, 18, 4, 64, 1, 20)).collect();
        assert_eq!(widths, vec![42, 39, 34, 29, 27, 26, 25, 24]);
    }
}
