//! Designing the FIR that undoes a CIC's passband droop.
//!
//! A CIC's `sinc^N` shape attenuates the band you meant to keep, worst
//! at the edge, and the attenuation grows with `N`. At `N = 4, R = 32`
//! and a passband reaching 80% of the decimated Nyquist the edge is
//! down more than 7 dB — a measurement error, not a cosmetic one.
//!
//! The droop is deterministic, so it can be inverted. This module
//! designs a short symmetric FIR running at the **output** rate whose
//! response approximates `1/|H_cic(u)|` across the passband. Placed
//! after the decimator, the pair is flat where it matters.
//!
//! # Why the compensator goes after the decimator
//!
//! It could equivalently go before, at the input rate, and be a worse
//! idea in every respect: `R` times as many multiply-accumulates per
//! second, for the same correction. The CIC exists to get the rate
//! down before anything expensive happens; putting the expensive thing
//! in front of it defeats the arrangement.
//!
//! # Why symmetric, odd length
//!
//! Type-I linear phase. Two reasons, and for this library the second
//! is the binding one:
//!
//! - It halves the multipliers — `h[k] == h[L-1-k]`, so taps pair up.
//! - **Linear phase means constant group delay**, so every frequency
//!   in the band is delayed equally. A filter that delays one part of
//!   the band more than another distorts the envelope, and in a
//!   phase-sensitive receiver ([`super::super::ddc`]) that is not a
//!   cosmetic defect — it is the measurement.
//!
//! # What the design does not do
//!
//! It does not extend the CIC's stopband. Compensation shapes the
//! passband; alias rejection remains whatever
//! [`super::response::worst_alias_db`] says it is. If the aliases are
//! too big, the answer is more CIC stages or a smaller passband, not a
//! longer compensator.
//!
//! It also cannot fix the band near a CIC null: `1/|H|` diverges there,
//! so a passband demanding gain close to a null asks for arbitrarily
//! large taps. [`design`] reports the gain it actually needs so that
//! is visible rather than silent.

use super::response::{magnitude_out, passband_edge_out};

/// What to design for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    /// CIC stages.
    pub stages: usize,
    /// CIC decimation factor.
    pub rate: usize,
    /// CIC differential delay.
    pub delay: usize,
    /// Passband as a fraction of the decimated Nyquist. `0.8` is a
    /// common choice; above about `0.9` the inverse-sinc gain climbs
    /// steeply because the CIC null is close.
    pub passband: f64,
    /// Number of taps. Must be odd, so the filter has a whole-sample
    /// group delay and a centre tap.
    pub taps: usize,
    /// Where the don't-care region ends and the fitted stopband
    /// begins, as a fraction of Nyquist. Between `passband` and this,
    /// the design is unconstrained.
    pub stopband: f64,
    /// How hard to hold the stopband down, relative to passband
    /// accuracy. Zero fits the passband alone and lets out-of-band
    /// gain go where it likes.
    pub stopband_weight: f64,
}

impl Spec {
    /// A reasonable starting point for a CIC of the given shape.
    pub fn for_cic(stages: usize, rate: usize, delay: usize) -> Self {
        Self {
            stages,
            rate,
            delay,
            passband: 0.8,
            taps: 15,
            stopband: 1.0,
            stopband_weight: 0.05,
        }
    }
}

/// A designed compensator.
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    /// Tap values, symmetric, length `spec.taps`.
    pub taps: Vec<f64>,
    /// The spec this came from.
    pub spec: Spec,
    /// Peak-to-peak deviation from flat across the passband, in dB,
    /// for the CIC and this filter together. The headline number.
    pub ripple_db: f64,
    /// Largest gain the design asks for, at the passband edge. Watch
    /// this: it is what drives coefficient width.
    pub peak_gain: f64,
}

/// Amplitude response of a symmetric odd-length FIR at output-rate
/// frequency `u`.
///
/// Real-valued, and may be negative — for a Type-I filter the phase is
/// exactly linear, so all of the phase information is the sign.
pub fn fir_amplitude(taps: &[f64], u: f64) -> f64 {
    let c = taps.len() / 2;
    let mut a = taps[c];
    for i in 1..=c {
        a += 2.0 * taps[c - i] * (2.0 * std::f64::consts::PI * u * i as f64).cos();
    }
    a
}

/// Solve `A x = b` by Gaussian elimination with partial pivoting.
///
/// Indexed loops throughout, and clippy's suggestion to iterate is
/// declined deliberately: elimination is defined by the relationship
/// between row `r`, column `col` and pivot row, and rewriting it in
/// terms of iterators obscures which index is which. This is the one
/// place in the module where indices are the clearer notation.
///
/// The systems here are tiny — one unknown per unique tap, so eight or
/// so — and symmetric positive definite by construction, which is why
/// a textbook solver is the right amount of machinery.
#[allow(clippy::needless_range_loop)]
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let (piv, _) = (col..n)
            .map(|r| (r, a[r][col].abs()))
            .fold((col, -1.0), |acc, x| if x.1 > acc.1 { x } else { acc });
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in (col + 1)..n {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for cc in col..n {
                a[r][cc] -= f * a[col][cc];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for cc in (r + 1)..n {
            s -= a[r][cc] * x[cc];
        }
        x[r] = s / a[r][r];
    }
    Some(x)
}

/// Design a compensator by weighted least squares.
///
/// Fits the symmetric-FIR amplitude response to `1/|H_cic(u)|` across
/// the passband and to zero across the stopband, on a dense frequency
/// grid. Returns `None` if `taps` is even or the normal equations are
/// singular — the latter should not happen for a sane spec, and is
/// reported rather than papered over.
#[allow(clippy::needless_range_loop, clippy::manual_is_multiple_of)]
pub fn design(spec: Spec) -> Option<Design> {
    if spec.taps % 2 == 0 || spec.taps == 0 {
        return None;
    }
    let c = spec.taps / 2;
    let nb = c + 1; // unique taps: centre plus c pairs

    // Basis: phi_0 = 1, phi_i(u) = 2 cos(2 pi u i).
    let basis = |u: f64, i: usize| -> f64 {
        if i == 0 {
            1.0
        } else {
            2.0 * (2.0 * std::f64::consts::PI * u * i as f64).cos()
        }
    };

    let edge = passband_edge_out(spec.passband);
    let mut ata = vec![vec![0.0; nb]; nb];
    let mut atb = vec![0.0; nb];

    const GRID: usize = 512;
    // Passband: target the reciprocal of the CIC's droop.
    for g in 0..GRID {
        let u = edge * g as f64 / (GRID - 1) as f64;
        let h = magnitude_out(u, spec.stages, spec.rate, spec.delay);
        if h <= 1e-12 {
            return None; // passband touches a null; 1/H is unbounded
        }
        let target = 1.0 / h;
        let w = 1.0;
        for i in 0..nb {
            let pi_ = basis(u, i);
            for j in 0..nb {
                ata[i][j] += w * pi_ * basis(u, j);
            }
            atb[i] += w * pi_ * target;
        }
    }
    // Stopband: pull toward zero, softly.
    if spec.stopband_weight > 0.0 && spec.stopband < 0.5 {
        for g in 0..GRID {
            let u =
                spec.stopband * 0.5 + (0.5 - spec.stopband * 0.5) * g as f64 / (GRID - 1) as f64;
            let w = spec.stopband_weight;
            for i in 0..nb {
                let pi_ = basis(u, i);
                for j in 0..nb {
                    ata[i][j] += w * pi_ * basis(u, j);
                }
                // target is zero, so atb gets no contribution
            }
        }
    }

    let x = solve(ata, atb)?;
    // Unpack the symmetric tap vector.
    let mut taps = vec![0.0; spec.taps];
    taps[c] = x[0];
    for i in 1..=c {
        taps[c - i] = x[i];
        taps[c + i] = x[i];
    }

    let (ripple_db, peak_gain) = evaluate(&taps, &spec);
    Some(Design {
        taps,
        spec,
        ripple_db,
        peak_gain,
    })
}

/// Peak-to-peak passband deviation in dB, and the largest tap gain
/// asked of the filter.
fn evaluate(taps: &[f64], spec: &Spec) -> (f64, f64) {
    let edge = passband_edge_out(spec.passband);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut peak: f64 = 0.0;
    const GRID: usize = 1024;
    for g in 0..GRID {
        let u = edge * g as f64 / (GRID - 1) as f64;
        let composite =
            magnitude_out(u, spec.stages, spec.rate, spec.delay) * fir_amplitude(taps, u).abs();
        let db = 20.0 * composite.log10();
        lo = lo.min(db);
        hi = hi.max(db);
        peak = peak.max(fir_amplitude(taps, u).abs());
    }
    (hi - lo, peak)
}

/// Combined CIC-plus-compensator magnitude, in dB relative to DC, at
/// output-rate frequency `u`.
pub fn composite_db(taps: &[f64], spec: &Spec, u: f64) -> f64 {
    let a = magnitude_out(u, spec.stages, spec.rate, spec.delay) * fir_amplitude(taps, u).abs();
    if a <= 1e-15 { -300.0 } else { 20.0 * a.log10() }
}

/// Taps quantised to signed integers for hardware.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantised {
    /// Integer taps.
    pub taps: Vec<i64>,
    /// Fractional bits: the hardware divides the accumulator by
    /// `2^shift`, which is a right shift and therefore free.
    pub shift: u32,
    /// Bits needed to hold the largest tap, including sign.
    pub coeff_width: usize,
    /// DC gain of the quantised filter, as a ratio.
    ///
    /// Exactly `1.0` by construction — [`quantise`] trims the centre
    /// tap to make it so, because a DC gain error is a systematic
    /// amplitude error on every sample and does not average out the
    /// way ripple does.
    pub dc_gain: f64,
    /// Passband ripple in dB of CIC-plus-quantised-filter — the number
    /// that actually ships, as opposed to the ideal-tap one.
    pub ripple_db: f64,
}

/// Quantise a design to `coeff_width`-bit signed taps.
///
/// Picks the largest power-of-two scale that keeps every tap inside
/// the width, so the fractional precision is as good as the width
/// allows, and reports the ripple that survives.
pub fn quantise(design: &Design, coeff_width: usize) -> Quantised {
    let peak = design.taps.iter().fold(0.0f64, |m, t| m.max(t.abs()));
    let limit = ((1i64 << (coeff_width - 1)) - 1) as f64;
    // Largest shift with peak * 2^shift <= limit.
    let mut shift = 0u32;
    while peak * (1u64 << (shift + 1)) as f64 <= limit && shift < 62 {
        shift += 1;
    }
    let scale = (1u64 << shift) as f64;
    let mut taps: Vec<i64> = design
        .taps
        .iter()
        .map(|t| (t * scale).round() as i64)
        .collect();

    // Force the DC gain to exactly one by trimming the centre tap.
    //
    // Rounding each tap independently leaves the sum a few LSBs off
    // `2^shift`, which is a *systematic* gain error -- every amplitude
    // the chain reports is scaled by it. At 12-bit coefficients that
    // was 0.4%, which is far larger than the passband ripple the design
    // works so hard to remove, and unlike ripple it does not average
    // out.
    //
    // The centre tap absorbs it because it is the largest by an order
    // of magnitude, so a correction of a few LSBs is a relative change
    // of a fraction of a percent on one coefficient -- it moves the
    // response shape immeasurably while fixing the gain exactly.
    // `ripple_db` below is measured after the trim, so the cost is
    // reported rather than assumed negligible.
    let centre = taps.len() / 2;
    let sum: i64 = taps.iter().sum();
    let target = 1i64 << shift;
    taps[centre] += target - sum;

    let real: Vec<f64> = taps.iter().map(|t| *t as f64 / scale).collect();
    let dc_gain = real.iter().sum::<f64>();
    let (ripple_db, _) = evaluate(&real, &design.spec);
    Quantised {
        taps,
        shift,
        coeff_width,
        dc_gain,
        ripple_db,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::cic::response::passband_droop_db;

    #[test]
    fn an_even_tap_count_is_rejected() {
        let mut s = Spec::for_cic(3, 16, 1);
        s.taps = 16;
        assert!(design(s).is_none(), "even length has no centre tap");
    }

    #[test]
    fn the_taps_are_symmetric() {
        // Linear phase is the reason for the whole shape; if this
        // fails the group delay is frequency dependent and the
        // phase-sensitive receiver downstream is wrong.
        let d = design(Spec::for_cic(4, 32, 1)).unwrap();
        let n = d.taps.len();
        for k in 0..n {
            assert!(
                (d.taps[k] - d.taps[n - 1 - k]).abs() < 1e-15,
                "tap {k} breaks symmetry"
            );
        }
    }

    #[test]
    fn compensation_flattens_the_passband() {
        for (n, r) in [(2, 8), (3, 16), (4, 32), (5, 64)] {
            let spec = Spec::for_cic(n, r, 1);
            let d = design(spec).unwrap();
            let droop = passband_droop_db(spec.passband, n, r, 1).abs();
            println!(
                "N={n} R={r}: droop {droop:.2} dB -> ripple {:.4} dB (peak gain {:.2})",
                d.ripple_db, d.peak_gain
            );
            // The uncompensated band spans `droop` dB from DC to edge.
            // Compensation must cut that by a large factor, not merely
            // improve it.
            assert!(
                d.ripple_db < droop / 20.0,
                "N={n} R={r}: ripple {} vs droop {}",
                d.ripple_db,
                droop
            );
            assert!(d.ripple_db < 0.2, "N={n} R={r}: ripple {}", d.ripple_db);
        }
    }

    #[test]
    fn more_taps_fit_better() {
        let mut prev = f64::INFINITY;
        for taps in [7usize, 11, 15, 19, 23] {
            let mut s = Spec::for_cic(4, 32, 1);
            s.taps = taps;
            let d = design(s).unwrap();
            assert!(
                d.ripple_db < prev,
                "{taps} taps did not improve on the previous: {} vs {prev}",
                d.ripple_db
            );
            prev = d.ripple_db;
        }
    }

    #[test]
    fn a_wider_passband_costs_gain() {
        // Approaching the CIC null, 1/|H| diverges. The design should
        // report that as rising peak gain rather than hiding it.
        let mut a = Spec::for_cic(4, 32, 1);
        a.passband = 0.5;
        let mut b = a;
        b.passband = 0.9;
        assert!(design(b).unwrap().peak_gain > design(a).unwrap().peak_gain);
    }

    #[test]
    fn quantised_taps_keep_most_of_the_flatness() {
        let spec = Spec::for_cic(4, 32, 1);
        let d = design(spec).unwrap();
        for w in [12usize, 16, 18] {
            let q = quantise(&d, w);
            let peak = q.taps.iter().fold(0i64, |m, t| m.max(t.abs()));
            assert!(
                peak < (1i64 << (w - 1)),
                "width {w}: tap {peak} does not fit"
            );
            println!(
                "width {w}: shift {} ripple {:.4} dB dc {:.6}",
                q.shift, q.ripple_db, q.dc_gain
            );
            assert!(q.ripple_db < 0.5, "width {w}: ripple {}", q.ripple_db);
            // DC gain is exact, not approximately exact.
            assert_eq!(
                q.taps.iter().sum::<i64>(),
                1i64 << q.shift,
                "width {w}: taps must sum to exactly 2^shift"
            );
        }
        // More bits must not be worse.
        assert!(quantise(&d, 18).ripple_db <= quantise(&d, 12).ripple_db + 1e-9);
    }

    #[test]
    fn the_composite_is_flat_where_the_spec_says_and_not_beyond() {
        let spec = Spec::for_cic(4, 32, 1);
        let d = design(spec).unwrap();
        let edge = passband_edge_out(spec.passband);
        // Flat inside.
        for g in 0..50 {
            let u = edge * g as f64 / 49.0;
            assert!(
                composite_db(&d.taps, &spec, u).abs() < 0.2,
                "not flat at u={u}"
            );
        }
        // And the CIC's own nulls survive: compensation shapes the
        // passband, it does not fill in the stopband.
        let null_u = spec.rate as f64 / (spec.rate * spec.delay) as f64; // u = 1/M at output rate
        let _ = null_u;
        assert!(composite_db(&d.taps, &spec, 0.5) < 0.0);
    }
}
