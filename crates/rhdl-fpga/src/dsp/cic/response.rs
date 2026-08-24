//! Frequency response of a CIC decimator, in closed form.
//!
//! Everything a CIC costs you and everything it buys you is in one
//! expression. For an `N`-stage cascade decimating by `R` with
//! differential delay `M`, at frequency `f` normalised to the **input**
//! sample rate,
//!
//! ```text
//!            |  sin(pi f R M)  | N
//! |H(f)|  =  | --------------- |        normalised so |H(0)| = 1
//!            |    sin(pi f)    |
//! ```
//!
//! Two consequences, and they pull in opposite directions.
//!
//! **The good one.** `|H|` is zero at every `f = k/(RM)`, and those are
//! exactly the frequencies that decimation by `R` folds onto DC. A CIC
//! puts its nulls precisely where the aliases come from, which is why
//! it is the right filter in front of a decimator and why it needs no
//! coefficients to do it.
//!
//! **The bad one.** The same `sinc^N` shape droops across the
//! passband — the band you meant to keep is attenuated, increasingly
//! so toward its edge, and increasingly so with `N`. At `N = 4` and a
//! passband reaching 40% of the output Nyquist the droop is over a
//! decibel, which is not a rounding error in a measurement.
//!
//! That droop is deterministic and known ahead of time, so it can be
//! undone by a short FIR at the *output* rate — see
//! [`super::compensator`]. This module is what tells that design what
//! it has to invert, and what tells you whether the result is good
//! enough.
//!
//! # Frequency conventions, because mixing them up is the classic error
//!
//! Two normalisations appear here and they differ by a factor of `R`:
//!
//! - **Input-rate**, written `f`: cycles per input sample, Nyquist at
//!   `0.5`. The formula above is in these units. Use for null
//!   placement and alias analysis.
//! - **Output-rate**, written `u`: cycles per *output* sample, Nyquist
//!   at `0.5`. Use for anything the compensator sees, because the
//!   compensator runs after decimation.
//!
//! `u = f * R`. Functions are named for the one they take.

use std::f64::consts::PI;

/// Normalised magnitude response at input-rate frequency `f`.
///
/// `|H(0)| = 1`, so this is gain relative to DC rather than the raw
/// `(R·M)^N`. Returns exactly `0.0` at the nulls.
pub fn magnitude(f: f64, n: usize, r: usize, m: usize) -> f64 {
    let rm = (r * m) as f64;
    let num = PI * f * rm;
    let den = PI * f;
    // The ratio is R*M in the limit, and both sin() terms vanish there.
    // Taking the limit explicitly rather than letting 0/0 through is
    // not defensive coding -- f = 0 is the normalisation point and the
    // single most likely argument.
    let ratio = if den.abs() < 1e-12 {
        rm
    } else {
        num.sin() / den.sin()
    };
    (ratio.abs() / rm).powi(n as i32)
}

/// Magnitude in decibels, floored at `-300` so nulls stay plottable.
pub fn magnitude_db(f: f64, n: usize, r: usize, m: usize) -> f64 {
    let a = magnitude(f, n, r, m);
    if a <= 1e-15 { -300.0 } else { 20.0 * a.log10() }
}

/// Normalised magnitude at output-rate frequency `u`.
///
/// What the compensator has to invert: `u` is in cycles per output
/// sample, so `u = 0.5` is the decimated Nyquist.
pub fn magnitude_out(u: f64, n: usize, r: usize, m: usize) -> f64 {
    magnitude(u / r as f64, n, r, m)
}

/// Passband edge in output-rate units, from a fraction of output
/// Nyquist.
///
/// `passband = 0.8` means "the band I care about reaches 80% of the
/// decimated Nyquist", which is `u = 0.4`.
pub fn passband_edge_out(passband: f64) -> f64 {
    0.5 * passband
}

/// Droop at the passband edge, in dB. Negative — it is attenuation.
///
/// This is the number the compensator exists to remove, and the honest
/// headline for "how bad is an uncompensated CIC here".
pub fn passband_droop_db(passband: f64, n: usize, r: usize, m: usize) -> f64 {
    20.0 * magnitude_out(passband_edge_out(passband), n, r, m).log10()
}

/// Worst-case gain, in dB, anywhere that decimation folds into the
/// passband.
///
/// Decimating by `R` maps every band around `f = k/R` onto baseband.
/// Energy there lands on top of the signal and cannot be separated
/// afterwards, so the largest `|H|` across those bands is the CIC's
/// real anti-alias figure — not the depth of its nulls, which is
/// infinite and therefore meaningless on its own.
///
/// Returns the maximum over `k = 1..=R/2` of `|H|` on
/// `|f - k/R| <= passband_edge/R`.
pub fn worst_alias_db(passband: f64, n: usize, r: usize, m: usize) -> f64 {
    let edge = passband_edge_out(passband) / r as f64;
    let mut worst: f64 = 0.0;
    for k in 1..=(r / 2) {
        let centre = k as f64 / r as f64;
        // Sample the band densely; the extremum sits at one edge in
        // practice, but sampling avoids assuming which.
        const STEPS: usize = 257;
        for s in 0..STEPS {
            let f = centre - edge + 2.0 * edge * (s as f64 / (STEPS - 1) as f64);
            if f <= 0.0 || f > 0.5 {
                continue;
            }
            worst = worst.max(magnitude(f, n, r, m));
        }
    }
    if worst <= 1e-15 {
        -300.0
    } else {
        20.0 * worst.log10()
    }
}

/// The response sampled across the whole input band, for plotting.
///
/// Returns `(f, dB)` pairs on `[0, 0.5]`.
pub fn curve_input(points: usize, n: usize, r: usize, m: usize) -> Vec<(f64, f64)> {
    (0..points)
        .map(|k| {
            let f = 0.5 * k as f64 / (points - 1) as f64;
            (f, magnitude_db(f, n, r, m))
        })
        .collect()
}

/// The response across the decimated band, for plotting.
///
/// Returns `(u, dB)` pairs on `[0, 0.5]` in output-rate units — the
/// view the compensator works in.
pub fn curve_output(points: usize, n: usize, r: usize, m: usize) -> Vec<(f64, f64)> {
    (0..points)
        .map(|k| {
            let u = 0.5 * k as f64 / (points - 1) as f64;
            let a = magnitude_out(u, n, r, m);
            (u, if a <= 1e-15 { -300.0 } else { 20.0 * a.log10() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_gain_is_unity_by_construction() {
        for (n, r, m) in [(1, 2, 1), (2, 4, 1), (4, 64, 1), (3, 16, 2), (5, 1024, 1)] {
            assert!((magnitude(0.0, n, r, m) - 1.0).abs() < 1e-12, "{n} {r} {m}");
            assert!(magnitude_db(0.0, n, r, m).abs() < 1e-9);
        }
    }

    #[test]
    fn the_nulls_are_where_decimation_folds() {
        // f = k/(R*M) for every k that is not a multiple of R*M.
        for (n, r, m) in [(2, 4, 1), (3, 8, 1), (2, 16, 2)] {
            let rm = r * m;
            for k in 1..rm {
                let f = k as f64 / rm as f64;
                if f > 0.5 {
                    break;
                }
                assert!(
                    magnitude(f, n, r, m) < 1e-12,
                    "expected a null at f={f} for N={n} R={r} M={m}"
                );
            }
        }
    }

    #[test]
    fn the_response_falls_monotonically_to_the_first_null() {
        // Between DC and the first null there is no ripple -- a CIC's
        // passband is a droop, not a Chebyshev.
        let (n, r, m) = (4, 32, 1);
        let first_null = 1.0 / (r * m) as f64;
        let mut prev = f64::INFINITY;
        for k in 0..200 {
            let f = first_null * k as f64 / 200.0;
            let a = magnitude(f, n, r, m);
            assert!(a <= prev + 1e-15, "not monotonic at f={f}");
            prev = a;
        }
    }

    #[test]
    fn droop_worsens_with_depth_and_with_bandwidth() {
        let (r, m) = (32, 1);
        let a = passband_droop_db(0.4, 2, r, m);
        let b = passband_droop_db(0.4, 4, r, m);
        assert!(b < a, "more stages must droop more: {b} vs {a}");
        let c = passband_droop_db(0.8, 4, r, m);
        assert!(c < b, "a wider passband must droop more: {c} vs {b}");
        // And the depth relationship is exactly a power law: N stages
        // is the single-stage droop N times over, in dB.
        let one = passband_droop_db(0.4, 1, r, m);
        assert!((b - 4.0 * one).abs() < 1e-9, "{b} vs 4x{one}");
    }

    #[test]
    fn a_deeper_cascade_rejects_aliases_better() {
        let (r, m, pb) = (32, 1, 0.5);
        let a = worst_alias_db(pb, 2, r, m);
        let b = worst_alias_db(pb, 4, r, m);
        assert!(b < a, "more stages must reject more: {b} vs {a}");
    }

    /// The output-rate view is the input-rate view stretched by `R`.
    #[test]
    fn the_two_frequency_conventions_agree() {
        let (n, r, m) = (3, 16, 1);
        for k in 0..50 {
            let u = 0.5 * k as f64 / 49.0;
            assert!((magnitude_out(u, n, r, m) - magnitude(u / r as f64, n, r, m)).abs() < 1e-15);
        }
    }
}
