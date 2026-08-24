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

use super::response::passband_edge_out;

/// One CIC in the response being inverted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CicShape {
    /// This stage's decimation factor.
    pub decimate: usize,
    /// Integrator/comb pairs.
    pub stages: usize,
    /// Differential delay.
    pub delay: usize,
}

/// What to design for.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The CICs whose combined droop is to be inverted, in signal
    /// order.
    ///
    /// One entry is the ordinary case. **A cascade must list every
    /// stage**: inverting only the last one leaves the earlier stages'
    /// droop uncorrected, which is usually small but is not zero and is
    /// not something to assume. Each stage is evaluated at its own
    /// input rate — see [`cascade_magnitude`].
    pub cics: Vec<CicShape>,
    /// Passband as a fraction of the decimated Nyquist. Above about
    /// `0.9` the inverse-sinc gain climbs steeply, because the CIC null
    /// is close.
    pub passband: f64,
    /// Number of taps. Must be odd, so the filter has a whole-sample
    /// group delay and a centre tap.
    pub taps: usize,
    /// Where the stopband begins, as a fraction of Nyquist.
    ///
    /// Between `passband` and this the design is unconstrained — the
    /// transition band. A narrow transition costs taps.
    pub stopband_edge: f64,
    /// Required stopband attenuation, in dB, positive.
    ///
    /// **This is what makes the filter an anti-alias filter as well as
    /// a compensator.** A CIC's own stopband is whatever `sinc^N`
    /// happens to give, which is often not enough — and if anything
    /// downstream decimates further, the compensator is the natural
    /// place to put the attenuation, because it is already there and
    /// already running at the low rate.
    ///
    /// Zero means don't care: fit the passband alone and let
    /// out-of-band gain go where it likes.
    pub min_stopband_db: f64,
}

impl Spec {
    /// A reasonable starting point for a single CIC of the given shape.
    ///
    /// Compensation only — no stopband requirement. Add one by setting
    /// [`Spec::min_stopband_db`] and [`Spec::stopband_edge`].
    pub fn for_cic(stages: usize, decimate: usize, delay: usize) -> Self {
        Self {
            cics: vec![CicShape {
                decimate,
                stages,
                delay,
            }],
            passband: 0.8,
            taps: 15,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
        }
    }

    /// A starting point for a cascade.
    pub fn for_cascade(cics: Vec<CicShape>) -> Self {
        Self {
            cics,
            passband: 0.8,
            taps: 15,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
        }
    }

    /// Total decimation across the listed stages.
    pub fn total_decimate(&self) -> usize {
        self.cics.iter().map(|c| c.decimate).product()
    }
}

/// Combined normalised magnitude of a CIC cascade at output-rate `u`.
///
/// Each stage sees the same physical frequency, but normalised to its
/// *own* input rate — which differs by the product of the factors ahead
/// of it. Getting that scaling wrong is the classic error in cascade
/// analysis, so the running divisor is explicit here rather than folded
/// into an index.
pub fn cascade_magnitude(cics: &[CicShape], u: f64) -> f64 {
    let total: usize = cics.iter().map(|c| c.decimate).product();
    // Physical frequency, normalised to the first stage's input rate.
    let f = u / total as f64;
    let mut mag = 1.0;
    let mut ahead = 1usize;
    for c in cics {
        mag *= super::response::magnitude(f * ahead as f64, c.stages, c.decimate, c.delay);
        ahead *= c.decimate;
    }
    mag
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
    /// Worst-case stopband attenuation achieved, in dB, positive.
    ///
    /// The filter alone, not the CIC-plus-filter: the CIC's own
    /// stopband is reported by
    /// [`super::response::worst_alias_db`], and confusing the two
    /// flatters the result.
    pub stopband_db: f64,
    /// Weight the search settled on for the stopband term. Reported
    /// because it is the knob, and a large value means flatness was
    /// traded away for attenuation.
    pub stopband_weight: f64,
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
/// Fits the symmetric-FIR amplitude response to `1/|H(u)|` across the
/// passband, where `H` is the *combined* response of every CIC in
/// [`Spec::cics`], and to zero across the stopband.
///
/// # The stopband weight is searched, not chosen
///
/// A least-squares fit balances passband accuracy against stopband
/// attenuation through one weight, and the right value depends on the
/// tap count, the passband width and how deep the stopband has to be —
/// there is no good constant. So the weight is escalated along a
/// geometric ladder and the *smallest* one meeting
/// [`Spec::min_stopband_db`] is taken, because every increment beyond
/// what the stopband needs is ripple given away for nothing.
///
/// With `min_stopband_db == 0` the ladder is skipped: pure
/// compensation, no attenuation requirement, weight zero.
///
/// Returns `None` if `taps` is even, the passband touches a null (where
/// `1/|H|` is unbounded), or the normal equations are singular.
#[allow(clippy::needless_range_loop, clippy::manual_is_multiple_of)]
pub fn design(spec: Spec) -> Option<Design> {
    if spec.taps % 2 == 0 || spec.taps == 0 || spec.cics.is_empty() {
        return None;
    }
    if spec.min_stopband_db <= 0.0 {
        // Pure compensation: no stopband term at all.
        return design_at_weight(&spec, 0.0);
    }

    // Attenuation rises monotonically with the stopband weight, so the
    // least sufficient rung is found by bisection rather than by
    // walking the ladder. That matters: a linear scan of 33 rungs is 33
    // full least-squares fits *per tap count*, and a chain designer
    // tries a dozen tap counts across several splits. Bisection turns
    // ~33 fits into ~6, which took one exploratory run from 70 seconds
    // to a few.
    //
    // Monotonicity is an empirical property of the fit, not a theorem,
    // so the result is *verified* against the requirement before being
    // returned -- bisection on a non-monotonic function would otherwise
    // hand back something that misses the spec.
    const RUNGS: usize = 33;
    let weight_of = |k: usize| 1e-3 * 10f64.powf(k as f64 / 8.0);

    let top = design_at_weight(&spec, weight_of(RUNGS - 1))?;
    if top.stopband_db < spec.min_stopband_db {
        // Not reachable at any weight; report the deepest attempt so
        // the caller sees how far short it fell.
        return Some(top);
    }
    let mut lo = 0usize; // may not suffice
    let mut hi = RUNGS - 1; // known to suffice
    let mut best = top;
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let d = design_at_weight(&spec, weight_of(mid))?;
        if d.stopband_db >= spec.min_stopband_db {
            hi = mid;
            best = d;
        } else {
            lo = mid;
        }
    }
    // Rung zero might already have been enough.
    if hi == 1 {
        let d = design_at_weight(&spec, weight_of(0))?;
        if d.stopband_db >= spec.min_stopband_db {
            best = d;
        }
    }
    debug_assert!(best.stopband_db >= spec.min_stopband_db);
    Some(best)
}

/// One least-squares fit at a fixed stopband weight.
#[allow(clippy::needless_range_loop)]
fn design_at_weight(spec: &Spec, weight: f64) -> Option<Design> {
    let c = spec.taps / 2;
    let nb = c + 1;
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
    for g in 0..GRID {
        let u = edge * g as f64 / (GRID - 1) as f64;
        let h = cascade_magnitude(&spec.cics, u);
        if h <= 1e-12 {
            return None; // the passband touches a null
        }
        let target = 1.0 / h;
        for i in 0..nb {
            let pi_ = basis(u, i);
            for j in 0..nb {
                ata[i][j] += pi_ * basis(u, j);
            }
            atb[i] += pi_ * target;
        }
    }
    let stop_lo = 0.5 * spec.stopband_edge;
    if weight > 0.0 && stop_lo < 0.5 {
        for g in 0..GRID {
            let u = stop_lo + (0.5 - stop_lo) * g as f64 / (GRID - 1) as f64;
            for i in 0..nb {
                let pi_ = basis(u, i);
                for j in 0..nb {
                    ata[i][j] += weight * pi_ * basis(u, j);
                }
                // target is zero, so `atb` gets nothing
            }
        }
    }

    let x = solve(ata, atb)?;
    let mut taps = vec![0.0; spec.taps];
    taps[c] = x[0];
    for i in 1..=c {
        taps[c - i] = x[i];
        taps[c + i] = x[i];
    }

    let (ripple_db, peak_gain) = evaluate(&taps, spec);
    Some(Design {
        stopband_db: stopband_db(&taps, spec),
        ripple_db,
        peak_gain,
        stopband_weight: weight,
        taps,
        spec: spec.clone(),
    })
}

/// Worst-case stopband attenuation of the filter alone, in dB positive.
fn stopband_db(taps: &[f64], spec: &Spec) -> f64 {
    let lo = 0.5 * spec.stopband_edge;
    if lo >= 0.5 {
        return f64::INFINITY;
    }
    let mut worst: f64 = 0.0;
    const GRID: usize = 512;
    for g in 0..GRID {
        let u = lo + (0.5 - lo) * g as f64 / (GRID - 1) as f64;
        worst = worst.max(fir_amplitude(taps, u).abs());
    }
    if worst <= 1e-15 {
        f64::INFINITY
    } else {
        -20.0 * worst.log10()
    }
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
        let composite = cascade_magnitude(&spec.cics, u) * fir_amplitude(taps, u).abs();
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
    let a = cascade_magnitude(&spec.cics, u) * fir_amplitude(taps, u).abs();
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
    /// Stopband attenuation in dB of the quantised filter alone.
    ///
    /// Quantisation raises a stopband floor: rounded taps cannot cancel
    /// as precisely as exact ones, and deep stopbands are where that
    /// shows first. If this is well short of the ideal design's figure,
    /// the coefficient width is the limit.
    pub stopband_db: f64,
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
        stopband_db: stopband_db(&real, &design.spec),
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
            let d = design(spec.clone()).unwrap();
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
        let mut b = a.clone();
        b.passband = 0.9;
        assert!(design(b).unwrap().peak_gain > design(a).unwrap().peak_gain);
    }

    #[test]
    fn quantised_taps_keep_most_of_the_flatness() {
        let spec = Spec::for_cic(4, 32, 1);
        let d = design(spec).unwrap();
        #[allow(clippy::needless_range_loop)]
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
        let d = design(spec.clone()).unwrap();
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
        assert!(composite_db(&d.taps, &spec, 0.5) < 0.0);
    }
}

#[cfg(test)]
mod cascade_and_stopband_tests {
    use super::*;

    fn shape(decimate: usize, stages: usize, delay: usize) -> CicShape {
        CicShape {
            decimate,
            stages,
            delay,
        }
    }

    /// A one-stage cascade is the single-CIC case, exactly.
    #[test]
    fn one_stage_matches_the_plain_response() {
        let cics = vec![shape(32, 4, 1)];
        for k in 0..40 {
            let u = 0.5 * k as f64 / 39.0;
            let a = cascade_magnitude(&cics, u);
            let b = super::super::response::magnitude_out(u, 4, 32, 1);
            assert!((a - b).abs() < 1e-15, "u={u}: {a} vs {b}");
        }
    }

    /// **Each stage is evaluated at its own input rate.**
    ///
    /// The classic cascade error is to normalise every stage to the
    /// converter rate, which understates the later stages' droop by the
    /// factors ahead of them. Two orderings of the same factors are
    /// different filters, and if the scaling were dropped they would
    /// come out identical.
    #[test]
    fn stage_order_changes_the_response() {
        let fwd = vec![shape(8, 2, 1), shape(61, 5, 1)];
        let rev = vec![shape(61, 5, 1), shape(8, 2, 1)];
        // Same at DC by construction...
        assert!((cascade_magnitude(&fwd, 0.0) - 1.0).abs() < 1e-12);
        assert!((cascade_magnitude(&rev, 0.0) - 1.0).abs() < 1e-12);
        // ...and different anywhere else.
        let mut differ = false;
        for k in 1..30 {
            let u = 0.5 * k as f64 / 29.0;
            if (cascade_magnitude(&fwd, u) - cascade_magnitude(&rev, u)).abs() > 1e-9 {
                differ = true;
            }
        }
        assert!(differ, "the ordering must matter away from DC");
    }

    /// Compensating a cascade must beat compensating only its last
    /// stage — otherwise listing every stage buys nothing.
    #[test]
    fn compensating_the_whole_cascade_beats_the_last_stage_alone() {
        let full = vec![shape(4, 3, 1), shape(16, 4, 1)];
        let both = Spec {
            cics: full.clone(),
            passband: 0.7,
            taps: 15,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
        };
        // Design for the last stage only, then measure it against the
        // *whole* cascade -- which is what a designer that ignored the
        // earlier stage would ship.
        let last_only = Spec {
            cics: vec![shape(16, 4, 1)],
            ..both.clone()
        };
        let a = design(both.clone()).unwrap();
        let b = design(last_only).unwrap();
        let measured_b = {
            let edge = passband_edge_out(both.passband);
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for g in 0..512 {
                let u = edge * g as f64 / 511.0;
                let db =
                    20.0 * (cascade_magnitude(&full, u) * fir_amplitude(&b.taps, u).abs()).log10();
                lo = lo.min(db);
                hi = hi.max(db);
            }
            hi - lo
        };
        println!(
            "whole cascade {:.5} dB, last stage only {:.5} dB",
            a.ripple_db, measured_b
        );
        assert!(
            a.ripple_db <= measured_b,
            "inverting every stage must be at least as flat: {} vs {}",
            a.ripple_db,
            measured_b
        );
    }

    /// With no stopband asked for, the weight stays zero.
    #[test]
    fn compensation_only_uses_no_stopband_weight() {
        let d = design(Spec::for_cic(4, 32, 1)).unwrap();
        assert_eq!(d.stopband_weight, 0.0);
    }

    /// **The stopband requirement is met, and costs ripple.**
    ///
    /// This is the anti-alias half of the filter's job: attenuation
    /// above a transition band, on top of inverting the droop.
    #[test]
    fn a_stopband_requirement_is_met() {
        let plain = Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps: 31,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
        };
        let filtering = Spec {
            stopband_edge: 0.7,
            min_stopband_db: 40.0,
            ..plain.clone()
        };
        let a = design(plain).unwrap();
        let b = design(filtering.clone()).unwrap();
        println!(
            "compensation only: ripple {:.4} dB, stopband {:.1} dB\\n\
             with anti-alias:   ripple {:.4} dB, stopband {:.1} dB (weight {:.3})",
            a.ripple_db, a.stopband_db, b.ripple_db, b.stopband_db, b.stopband_weight
        );
        assert!(
            b.stopband_db >= filtering.min_stopband_db,
            "asked for {} dB, got {}",
            filtering.min_stopband_db,
            b.stopband_db
        );
        assert!(b.stopband_weight > 0.0, "the weight must have been raised");
        // And it is not free: attenuation is bought with passband
        // accuracy at a fixed tap count.
        assert!(
            b.ripple_db >= a.ripple_db,
            "attenuation should cost ripple: {} vs {}",
            b.ripple_db,
            a.ripple_db
        );
    }

    /// The weight chosen is the smallest that suffices.
    #[test]
    fn the_weight_is_not_overspent() {
        let spec = Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps: 31,
            stopband_edge: 0.7,
            min_stopband_db: 30.0,
        };
        let d = design(spec.clone()).unwrap();
        // One rung down the ladder must fail the requirement, or the
        // search overspent and gave away ripple for nothing.
        let lower = d.stopband_weight / 10f64.powf(1.0 / 8.0);
        if lower >= 1e-3 {
            let weaker = design_at_weight(&spec, lower).unwrap();
            assert!(
                weaker.stopband_db < spec.min_stopband_db,
                "weight {} was more than needed: {} already gives {}",
                d.stopband_weight,
                lower,
                weaker.stopband_db
            );
        }
    }

    /// An impossible stopband is reported, not silently missed.
    #[test]
    fn an_unreachable_stopband_returns_its_best() {
        let spec = Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps: 5, // far too few for 90 dB
            stopband_edge: 0.55,
            min_stopband_db: 90.0,
        };
        let d = design(spec.clone()).expect("a design is still returned");
        assert!(
            d.stopband_db < spec.min_stopband_db,
            "this cannot have succeeded: {}",
            d.stopband_db
        );
        // The caller decides what to do; the designer reports honestly.
        assert!(d.stopband_db.is_finite());
    }

    /// Quantisation raises the stopband floor, and that is reported.
    #[test]
    fn quantisation_limits_the_stopband() {
        let spec = Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps: 31,
            stopband_edge: 0.7,
            min_stopband_db: 60.0,
        };
        let d = design(spec).unwrap();
        let narrow = quantise(&d, 10);
        let wide = quantise(&d, 20);
        println!(
            "ideal {:.1} dB, 10-bit {:.1} dB, 20-bit {:.1} dB",
            d.stopband_db, narrow.stopband_db, wide.stopband_db
        );
        assert!(
            wide.stopband_db >= narrow.stopband_db,
            "more coefficient bits must not reject less"
        );
    }
}
