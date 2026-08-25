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
    /// Peak-to-peak passband ripple the design aims at, in dB.
    ///
    /// Used by [`Method::Remez`], which needs both targets to fix its
    /// band weighting in closed form. Ignored by
    /// [`Method::LeastSquares`], which has no notion of a target — it
    /// minimises average error and you take what you get.
    pub max_ripple_db: f64,
    /// How to fit.
    pub method: Method,
}

/// How to fit the taps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Method {
    /// Weighted least squares: minimise *average* squared error.
    ///
    /// Fine for pure compensation, where there is no worst-case
    /// requirement to miss. Simple, always converges, and the stopband
    /// weight has to be searched because its relationship to achieved
    /// attenuation is empirical.
    #[default]
    LeastSquares,
    /// Remez exchange: minimise the *maximum* weighted error.
    ///
    /// The right method whenever a stopband attenuation is specified,
    /// because "at least 60 dB everywhere" is a statement about the
    /// maximum and least squares will trade a deep notch here for a
    /// shallow one there. Also needs no weight search: both dB targets
    /// fix the weighting analytically.
    ///
    /// Can fail to converge on a badly conditioned spec, and says so
    /// via [`Design::converged`] rather than returning something that
    /// looks fine.
    Remez,
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
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
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
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
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
    /// The **composite** — cascade and filter together, relative to DC.
    /// This is what decides whether out-of-band content survives into
    /// the output, so it is what the requirement is stated against.
    ///
    /// It used to be the filter alone. That was conservative rather than
    /// wrong, but it charged the compensator for rolloff the cascade
    /// already provides, and it created a perverse incentive: a deeper
    /// CIC needs more passband boost, that boost spills past the
    /// passband edge, and so *adding* CIC stages made this figure worse
    /// and the requirement harder to meet.
    ///
    /// Not to be confused with [`super::response::worst_alias_db`],
    /// which is the cascade's own worst-case gain at the frequencies
    /// decimation folds onto the passband — a different question about a
    /// different band.
    pub stopband_db: f64,
    /// Weight the search settled on for the stopband term. Reported
    /// because it is the knob, and a large value means flatness was
    /// traded away for attenuation.
    pub stopband_weight: f64,
    /// Which method produced these taps.
    pub method: Method,
    /// The equiripple weighted error the exchange converged to.
    ///
    /// Zero for least squares, which has no such quantity. For Remez
    /// this is the minimax error *on the design grid*: at convergence
    /// every grid extremum has exactly this magnitude.
    pub delta: f64,
    /// Did the fit converge?
    ///
    /// Always true for least squares, which is a single linear solve.
    /// For Remez it can be false, and then the taps are the best
    /// iterate rather than the optimum — worth knowing before shipping
    /// them.
    pub converged: bool,
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
    if spec.method == Method::Remez {
        return remez::design(&spec);
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
            // Weighting by `|H|^2` makes the quantity being minimised
            // the *composite* stopband energy rather than the filter's
            // own: the residual is `|H(u) * fir(u)|`, so the fit stops
            // paying to attenuate frequencies the cascade has already
            // dealt with. Squared, because this is a squared-error
            // normal-equation accumulation.
            //
            // No floor is needed here. Where the cascade nulls, the
            // weight vanishes and the frequency simply drops out of the
            // objective, which is the correct answer -- there is nothing
            // left to attenuate. The passband terms keep the system
            // non-singular.
            let h = cascade_magnitude(&spec.cics, u);
            let w = weight * h * h;
            for i in 0..nb {
                let pi_ = basis(u, i);
                for j in 0..nb {
                    ata[i][j] += w * pi_ * basis(u, j);
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
        method: Method::LeastSquares,
        delta: 0.0,
        converged: true,
        taps,
        spec: spec.clone(),
    })
}

/// Worst-case stopband attenuation of the **composite**, in dB positive.
///
/// The cascade and the compensator together, relative to DC — which is
/// the number that decides whether out-of-band content survives into
/// the output, and therefore the one worth specifying against.
///
/// This used to measure the filter alone. That was a conservative
/// figure rather than a wrong one, but it made the requirement harder
/// than the physics: the cascade contributes tens of dB of its own
/// rolloff above the stopband edge, and charging the compensator for
/// all of it spends taps on attenuation that already exists. It also
/// meant the reported figure described a filter nobody listens to on
/// its own — the composite sits well below it, so a reader comparing
/// the number to a plot of the composite concluded one of them was
/// wrong.
///
/// The reference is unity, not the composite's measured DC gain: the
/// passband target is `1/|H|`, so the composite is 1.0 at DC by
/// construction, and [`quantise`] trims the centre tap to keep it there
/// exactly.
fn stopband_db(taps: &[f64], spec: &Spec) -> f64 {
    let lo = 0.5 * spec.stopband_edge;
    if lo >= 0.5 {
        return f64::INFINITY;
    }
    let mut worst: f64 = 0.0;
    const GRID: usize = 512;
    for g in 0..GRID {
        let u = lo + (0.5 - lo) * g as f64 / (GRID - 1) as f64;
        worst = worst.max(cascade_magnitude(&spec.cics, u) * fir_amplitude(taps, u).abs());
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
    /// Composite stopband attenuation in dB, with the taps quantised.
    ///
    /// Quantisation raises a stopband floor: rounded taps cannot cancel
    /// as precisely as exact ones, and deep stopbands are where that
    /// shows first. If this is well short of the ideal design's figure,
    /// the coefficient width is the limit.
    ///
    /// Only while quantisation is what binds, though. Once it is not,
    /// this figure stops improving with width and wobbles by about a dB
    /// either way, because the composite's worst case sits in a narrow
    /// region near the stopband edge where a coefficient perturbation
    /// moves it at random. A width that scores slightly better than a
    /// wider one is that noise, not a finding.
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

/// Equiripple design by Remez exchange.
pub mod remez {
    /// Equiripple design by the Remez exchange algorithm.
    ///
    /// # Why this exists alongside least squares
    ///
    /// Least squares minimises the *average* squared error. A stopband
    /// requirement is about the *worst case*: "at least 60 dB everywhere
    /// above the transition" is a statement about the maximum, and a
    /// method that trades a deep notch here for a shallow one there
    /// satisfies the average while missing the specification. That is why
    /// 60 dB across a 0.5-to-0.7 transition needs 37 least-squares taps
    /// where Remez needs 29.
    ///
    /// Remez minimises the maximum weighted error directly, which is the
    /// quantity the specification is written in.
    ///
    /// # It is self-certifying
    ///
    /// By Chebyshev's alternation theorem, a length-`2M+1` linear-phase
    /// filter whose weighted error attains its maximum magnitude at `M+2`
    /// points with alternating sign **is** the optimal filter — there is no
    /// better one. So the test for this code is not "it beat least
    /// squares"; it is "the alternation condition holds", which establishes
    /// optimality without a reference implementation to compare against.
    ///
    /// # The weight is analytic, not searched
    ///
    /// The least-squares path bisects a stopband weight because the
    /// relationship between weight and achieved attenuation is empirical
    /// there. Here it is exact. With `W(u) = |H(u)|` across the passband
    /// the weighted error *is* the relative deviation `|A(u)·H(u) - 1|`,
    /// and with a constant `k` across the stopband the error is `k·|A(u)|`.
    /// Both equal the same `δ` at the optimum, so
    ///
    /// ```text
    /// passband deviation = δ                 (relative, so ±δ about unity)
    /// stopband gain      = δ / k
    /// ```
    ///
    /// Given a peak-to-peak passband ripple `Rp` in dB and a stopband
    /// attenuation `As` in dB:
    ///
    /// ```text
    /// δ_target = Rp / 17.372          since 20*log10((1+δ)/(1-δ)) ≈ 17.372·δ
    /// k        = δ_target · 10^(As/20)
    /// ```
    ///
    /// One design, no ladder, no bisection. If the achieved `δ` exceeds
    /// `δ_target` the tap count is short — which is a statement about the
    /// filter length rather than about the search.
    use super::{
        CicShape, Design, Method, Spec, cascade_magnitude, evaluate, fir_amplitude, solve,
    };

    /// Bands to approximate over.
    struct Bands {
        pass_hi: f64,
        stop_lo: f64,
        weight_stop: f64,
        /// Lower bound on the cascade magnitude used as a stopband
        /// weight. See [`Bands::at`].
        h_floor: f64,
    }

    impl Bands {
        /// Is `u` inside a specified band? Returns `(desired, weight)`, or
        /// `None` in the transition band where nothing is required.
        fn at(&self, u: f64, cics: &[CicShape]) -> Option<(f64, f64)> {
            if u <= self.pass_hi {
                let h = cascade_magnitude(cics, u);
                if h <= 1e-12 {
                    return None;
                }
                // Desired `1/H`, weighted by `H`, so the weighted error is
                // the *relative* deviation of the product from unity --
                // which is what "ripple" means. Weighting by 1 instead
                // would equalise absolute error, making the band edge (a
                // large `1/H`) look far worse than it is.
                Some((1.0 / h, h))
            } else if u >= self.stop_lo {
                // Weighted by `H` for the same reason the passband is:
                // it makes the weighted error `|H(u) * fir(u)|`, the
                // composite's own stopband level, so the equiripple
                // property lands on the composite rather than on a
                // filter nobody listens to alone. Uniform weighting
                // spends taps flattening the filter across frequencies
                // where the cascade is already 60 dB down.
                //
                // Floored, because the exchange solves with `delta / w`
                // and a CIC's stopband contains exact nulls. An
                // unfloored weight goes to zero there, the interpolation
                // becomes singular, and the design is rejected outright.
                // Once the cascade is `FLOOR_MARGIN_DB` past what was
                // asked for, treating it as exactly that far past costs
                // nothing real and keeps the system solvable.
                Some((
                    0.0,
                    self.weight_stop * cascade_magnitude(cics, u).max(self.h_floor),
                ))
            } else {
                None
            }
        }
    }

    /// How far past the requirement the cascade may be before its
    /// contribution stops being counted, in dB.
    ///
    /// Only a numerical guard: at 12 dB past, the frequency contributes
    /// 1/16th of the weight of one at the requirement, which is already
    /// negligible against the ones that bind.
    const FLOOR_MARGIN_DB: f64 = 12.0;

    /// Design an equiripple compensator.
    ///
    /// `taps` must be odd. Returns `None` if the system becomes singular —
    /// which in practice means the extremal set collapsed, and is reported
    /// rather than papered over.
    pub fn design(spec: &Spec) -> Option<Design> {
        let cics: &[CicShape] = &spec.cics;
        let taps = spec.taps;
        let passband = spec.passband;
        let stopband_edge = spec.stopband_edge;
        let ripple_db = spec.max_ripple_db;
        let stopband_db = spec.min_stopband_db;
        if taps % 2 == 0 || taps < 3 || cics.is_empty() {
            return None;
        }
        let m = taps / 2;
        let n_ext = m + 2;

        let delta_target = ripple_db / 17.372;
        let weight_stop = if stopband_db > 0.0 {
            delta_target * 10f64.powf(stopband_db / 20.0)
        } else {
            0.0
        };
        let bands = Bands {
            pass_hi: 0.25 * passband * 2.0, // passband edge in output-rate units
            stop_lo: 0.5 * stopband_edge,
            weight_stop,
            h_floor: 10f64.powf(-(stopband_db + FLOOR_MARGIN_DB) / 20.0),
        };

        // Dense grid, restricted to the specified bands.
        //
        // 32 points per extremum -- the textbook figure for
        // Parks-McClellan -- leaves the *continuous* error peaking about
        // 12% above the on-grid delta between samples, because the
        // weighted error near a band edge curves sharply where `1/H` is
        // largest. That is invisible to the exchange, which only sees
        // grid points, and it is exactly what an equiripple assertion
        // measured on a finer grid catches.
        const DENSITY: usize = 256;
        let grid: Vec<f64> = {
            let n = DENSITY * n_ext;
            (0..=n)
                .map(|k| 0.5 * k as f64 / n as f64)
                .filter(|u| bands.at(*u, cics).is_some())
                .collect()
        };
        if grid.len() < n_ext {
            return None;
        }

        // Initial extrema: evenly spread over the grid.
        let mut ext: Vec<usize> = (0..n_ext)
            .map(|i| i * (grid.len() - 1) / (n_ext - 1))
            .collect();

        let basis = |u: f64, i: usize| -> f64 {
            if i == 0 {
                1.0
            } else {
                2.0 * (2.0 * std::f64::consts::PI * u * i as f64).cos()
            }
        };

        let mut delta = 0.0;
        let mut coeffs = vec![0.0; m + 1];
        let mut converged = false;
        let mut iterations = 0;

        const MAX_ITER: usize = 60;
        for it in 0..MAX_ITER {
            iterations = it + 1;

            // Interpolation: A(u_j) + (-1)^j delta / W(u_j) = D(u_j).
            let mut a = vec![vec![0.0; m + 2]; n_ext];
            let mut b = vec![0.0; n_ext];
            for (j, gi) in ext.iter().enumerate() {
                let u = grid[*gi];
                let (d, w) = bands.at(u, cics)?;
                for i in 0..=m {
                    a[j][i] = basis(u, i);
                }
                let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
                // A zero weight means the band is unconstrained; that must
                // not become a division by zero.
                if w <= 0.0 {
                    return None;
                }
                a[j][m + 1] = sign / w;
                b[j] = d;
            }
            let x = solve(a, b)?;
            coeffs.copy_from_slice(&x[..=m]);
            let new_delta = x[m + 1];

            // Weighted error across the whole grid.
            let taps_now = symmetric_from(&coeffs, taps);
            let err = |u: f64| -> f64 {
                match bands.at(u, cics) {
                    None => 0.0,
                    Some((d, w)) => w * (fir_amplitude(&taps_now, u) - d),
                }
            };

            // Candidate extrema, found **per band**.
            //
            // The grid spans two bands separated by a transition gap.
            // Comparing a gap-adjacent point against its grid
            // neighbour reaches across that gap to a different band, so
            // the local-maximum test there is meaningless. An earlier
            // version worked around it by forcing every gap-adjacent
            // point into the candidate set unconditionally -- which
            // injected *non-extremal* points into the interpolation and
            // left the result 11% off equiripple.
            //
            // Endpoints of each band are always candidates, because the
            // error is free to peak there and within that band there is
            // nothing beyond them.
            let mut cand: Vec<usize> = Vec::new();
            {
                // Grid indices are contiguous within a band; a jump
                // larger than a step marks the transition.
                let step = 0.5 / (DENSITY * n_ext) as f64;
                let mut band_start = 0usize;
                for k in 0..grid.len() {
                    let ends_band = k + 1 == grid.len() || (grid[k + 1] - grid[k]) > 1.5 * step;
                    if !ends_band {
                        continue;
                    }
                    for i in band_start..=k {
                        let e = err(grid[i]).abs();
                        let at_edge = i == band_start || i == k;
                        let left = if i == band_start {
                            f64::NEG_INFINITY
                        } else {
                            err(grid[i - 1]).abs()
                        };
                        let right = if i == k {
                            f64::NEG_INFINITY
                        } else {
                            err(grid[i + 1]).abs()
                        };
                        if at_edge || (e >= left && e >= right) {
                            cand.push(i);
                        }
                    }
                    band_start = k + 1;
                }
            }

            // Collapse runs of the same sign, keeping the largest -- the
            // alternation set must alternate, and two same-sign neighbours
            // are one extremum sampled twice.
            let mut alt: Vec<usize> = Vec::new();
            for k in cand {
                let e = err(grid[k]);
                match alt.last() {
                    Some(prev) if (err(grid[*prev]) > 0.0) == (e > 0.0) => {
                        if e.abs() > err(grid[*prev]).abs() {
                            let n = alt.len();
                            alt[n - 1] = k;
                        }
                    }
                    _ => alt.push(k),
                }
            }

            // Trim to M+2 by dropping the weaker end, which preserves
            // alternation; growing beyond that means the grid found more
            // ripples than the filter has degrees of freedom.
            while alt.len() > n_ext {
                let first = err(grid[*alt.first().unwrap()]).abs();
                let last = err(grid[*alt.last().unwrap()]).abs();
                if first < last {
                    alt.remove(0);
                } else {
                    alt.pop();
                }
            }
            if alt.len() < n_ext {
                // The exchange has degenerated; report what we have rather
                // than looping on a bad set.
                delta = new_delta.abs();
                break;
            }

            // **Over the whole grid, not over the selected extrema.**
            // The alternation theorem's stopping condition is that no
            // frequency anywhere exceeds the interpolated delta -- so
            // measuring the peak only at the points already chosen
            // declares victory while a larger error sits at a point the
            // trim discarded. That produced an 11% spread in the
            // supposedly equiripple magnitudes.
            let peak = grid.iter().map(|u| err(*u).abs()).fold(0.0f64, f64::max);
            delta = new_delta.abs();
            ext = alt;

            // Converged when no grid point exceeds the interpolated delta
            // by more than a hair -- the alternation theorem's condition.
            if std::env::var("REMEZ_TRACE").is_ok() {
                eprintln!(
                    "  iter {it}: delta {delta:.6e} peak {peak:.6e} exts {} ratio {:.4}",
                    ext.len(),
                    peak / delta
                );
            }
            if peak <= delta * (1.0 + 1e-9) + 1e-15 {
                converged = true;
                break;
            }
        }

        let final_taps = symmetric_from(&coeffs, taps);
        let (ripple, peak_gain) = evaluate(&final_taps, spec);
        let _ = iterations;
        Some(Design {
            stopband_db: super::stopband_db(&final_taps, spec),
            ripple_db: ripple,
            peak_gain,
            // Remez fixes its weighting analytically from the two dB
            // targets, so there is no searched weight to report.
            stopband_weight: weight_stop,
            method: Method::Remez,
            delta,
            converged,
            taps: final_taps,
            spec: spec.clone(),
        })
    }

    /// The extremal frequencies and weighted errors at convergence.
    ///
    /// Exposed so a test can check the alternation condition directly:
    /// by Chebyshev's theorem, `M+2` extrema of equal magnitude and
    /// alternating sign *is* optimality, which certifies the result
    /// without a reference implementation to compare against.
    pub fn alternation(spec: &Spec) -> Option<Vec<f64>> {
        let d = design(spec)?;
        let bands = Bands {
            pass_hi: 0.25 * spec.passband * 2.0,
            stop_lo: 0.5 * spec.stopband_edge,
            h_floor: 10f64.powf(-(spec.min_stopband_db + FLOOR_MARGIN_DB) / 20.0),
            weight_stop: if spec.min_stopband_db > 0.0 {
                (spec.max_ripple_db / 17.372) * 10f64.powf(spec.min_stopband_db / 20.0)
            } else {
                0.0
            },
        };
        let n = spec.taps / 2 + 2;

        // **Per band, not across the whole axis.** At a band edge the
        // neighbour on one side lies across the transition gap, in a
        // different band, so a local-maximum test that reaches over the
        // gap compares unrelated quantities and silently drops the edge
        // extremum. Since the error usually *peaks* at a band edge,
        // dropping it loses the very points the alternation set needs --
        // which is how this helper first reported seven extrema for a
        // filter that has nine.
        let mut errs: Vec<f64> = Vec::new();
        for (lo, hi) in [(0.0, bands.pass_hi), (bands.stop_lo, 0.5)] {
            if hi <= lo {
                continue;
            }
            let steps = 64 * n;
            let pts: Vec<(f64, f64)> = (0..=steps)
                .map(|k| lo + (hi - lo) * k as f64 / steps as f64)
                .filter_map(|u| {
                    bands
                        .at(u, &spec.cics)
                        .map(|(des, w)| (u, w * (fir_amplitude(&d.taps, u) - des)))
                })
                .collect();
            if pts.is_empty() {
                continue;
            }
            for i in 0..pts.len() {
                let e = pts[i].1;
                // Endpoints count: the error is free to peak at a band
                // edge, and within this band there is nothing beyond it.
                let is_end = i == 0 || i + 1 == pts.len();
                let left = if i == 0 {
                    f64::NEG_INFINITY
                } else {
                    pts[i - 1].1.abs()
                };
                let right = if i + 1 == pts.len() {
                    f64::NEG_INFINITY
                } else {
                    pts[i + 1].1.abs()
                };
                if is_end || (e.abs() >= left && e.abs() >= right) {
                    match errs.last() {
                        Some(p) if (*p > 0.0) == (e > 0.0) => {
                            if e.abs() > p.abs() {
                                let m = errs.len();
                                errs[m - 1] = e;
                            }
                        }
                        _ => errs.push(e),
                    }
                }
            }
        }
        Some(errs)
    }

    /// Expand half-basis coefficients into a symmetric tap vector.
    fn symmetric_from(coeffs: &[f64], taps: usize) -> Vec<f64> {
        let c = taps / 2;
        let mut t = vec![0.0; taps];
        t[c] = coeffs[0];
        for i in 1..=c {
            t[c - i] = coeffs[i];
            t[c + i] = coeffs[i];
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::super::response::passband_droop_db;
    use super::*;

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
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
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
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
            min_stopband_db: 0.0,
        };
        let filtering = Spec {
            stopband_edge: 0.7,
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
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
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
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
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
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
    /// The reported figure is the composite's, measured independently.
    ///
    /// Guards the contract rather than the implementation: recompute the
    /// worst-case cascade-times-filter magnitude above the edge on a
    /// different grid and require it to agree.
    #[test]
    fn the_achieved_figure_is_the_composite() {
        let spec = Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps: 31,
            stopband_edge: 0.7,
            max_ripple_db: 0.1,
            method: Method::Remez,
            min_stopband_db: 50.0,
        };
        let d = design(spec.clone()).unwrap();

        let lo = 0.5 * spec.stopband_edge;
        let mut worst_composite: f64 = 0.0;
        let mut worst_filter: f64 = 0.0;
        const N: usize = 3001; // deliberately not the design grid
        for g in 0..N {
            let u = lo + (0.5 - lo) * g as f64 / (N - 1) as f64;
            let h = cascade_magnitude(&spec.cics, u);
            let f = fir_amplitude(&d.taps, u).abs();
            worst_composite = worst_composite.max(h * f);
            worst_filter = worst_filter.max(f);
        }
        let composite_db = -20.0 * worst_composite.log10();
        let filter_db = -20.0 * worst_filter.log10();
        println!(
            "reported {:.2} dB | composite {:.2} | filter alone {:.2}",
            d.stopband_db, composite_db, filter_db
        );
        assert!(
            (d.stopband_db - composite_db).abs() < 0.5,
            "reported {:.2} is not the composite {:.2}",
            d.stopband_db,
            composite_db
        );
        // And the composite is the more generous of the two, always:
        // the cascade only ever attenuates above its passband.
        assert!(
            composite_db > filter_db,
            "the composite cannot be worse than the filter alone: {composite_db:.2} vs {filter_db:.2}"
        );
    }

    /// Deepening the CIC must not make the stopband requirement harder.
    ///
    /// This is the reason the metric changed. Measured against the
    /// filter alone, more CIC stages made the spec *harder*: the deeper
    /// droop needs more passband boost, that boost spills past the
    /// passband edge, and the filter's own stopband suffers for it --
    /// N=5 needed 33 taps where N=4 needed 31, so the search was
    /// punished for using the cheap resource. Measured on the composite,
    /// the cascade's extra rolloff is credited against its own droop and
    /// the two very nearly cancel.
    ///
    /// The assertion is "no worse", not "better", because they do only
    /// cancel: extra CIC depth does not *buy* stopband, it stops costing
    /// it. Claiming otherwise would be claiming a win the numbers do not
    /// show.
    #[test]
    fn extra_cic_depth_does_not_cost_stopband() {
        let at_stages = |n: usize| {
            design(Spec {
                cics: vec![shape(32, n, 1)],
                passband: 0.5,
                taps: 29,
                stopband_edge: 0.7,
                max_ripple_db: 0.1,
                method: Method::Remez,
                min_stopband_db: 50.0,
            })
            .map(|d| d.stopband_db)
        };
        let shallow = at_stages(4).expect("N=4 designs");
        let deep = at_stages(6).expect("N=6 designs");
        println!("N=4 {shallow:.2} dB, N=6 {deep:.2} dB");
        assert!(
            deep >= shallow - 1.0,
            "extra depth cost {:.2} dB of stopband: {shallow:.2} -> {deep:.2}",
            shallow - deep
        );
    }

    /// The stopband weight is floored, so a CIC null cannot make the
    /// equiripple exchange singular.
    ///
    /// The weight is `weight_stop * |H(u)|`, and a CIC's stopband
    /// contains exact nulls where `|H|` is zero. The exchange solves
    /// with `delta / w`, so an unfloored weight rejects the design
    /// outright rather than returning a poor one. `stopband_edge` at 0.5
    /// puts several nulls inside the band.
    #[test]
    fn a_cic_null_inside_the_stopband_still_designs() {
        let d = design(Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.4,
            taps: 31,
            stopband_edge: 0.5,
            max_ripple_db: 0.1,
            method: Method::Remez,
            min_stopband_db: 40.0,
        });
        let d = d.expect("a null in the stopband must not reject the design");
        assert!(
            d.stopband_db.is_finite() && d.stopband_db > 0.0,
            "nonsense stopband figure: {}",
            d.stopband_db
        );
    }

    #[test]
    fn quantisation_limits_the_stopband() {
        let spec = Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps: 31,
            stopband_edge: 0.7,
            max_ripple_db: 0.1,
            method: Method::LeastSquares,
            min_stopband_db: 60.0,
        };
        let d = design(spec).unwrap();
        let at = |w: usize| quantise(&d, w).stopband_db;
        println!(
            "ideal {:.1} dB | 6-bit {:.1} | 8-bit {:.1} | 10-bit {:.1} | 16-bit {:.1} | 24-bit {:.1}",
            d.stopband_db,
            at(6),
            at(8),
            at(10),
            at(16),
            at(24)
        );

        // While quantisation is what binds, bits buy attenuation, and
        // steeply: 6 bits cannot hold a 50 dB stopband at all.
        assert!(
            at(6) < at(8) && at(8) < at(10),
            "coarse quantisation must be the binding limit: {:.1}, {:.1}, {:.1}",
            at(6),
            at(8),
            at(10)
        );
        assert!(
            at(24) - at(6) > 15.0,
            "24 bits should beat 6 by a wide margin: {:.1} vs {:.1}",
            at(24),
            at(6)
        );

        // Past that the *design* binds, and more bits stop helping. This
        // half used to be asserted as strict monotonicity across all
        // widths, which held only while the metric was the filter alone:
        // that figure is dominated by the quantisation noise floor
        // across the whole stopband, so it improved with every bit. The
        // composite's worst case sits in a narrow region near the
        // stopband edge where the cascade has not yet rolled off, and
        // there a coefficient perturbation moves the figure by a dB in
        // either direction at random -- 10 bits scored 52.8 dB against
        // 20 bits' 51.7. Asserting a trend through that noise was
        // asserting luck.
        for w in [12usize, 14, 16, 20, 24] {
            assert!(
                (at(w) - at(24)).abs() < 3.0,
                "{w} bits should be design-limited like 24, not {:.1} against {:.1}",
                at(w),
                at(24)
            );
        }
    }
}

#[cfg(test)]
mod remez_tests {
    use super::*;

    fn shape(decimate: usize, stages: usize, delay: usize) -> CicShape {
        CicShape {
            decimate,
            stages,
            delay,
        }
    }

    fn spec(taps: usize, stop_edge: f64, atten: f64) -> Spec {
        Spec {
            cics: vec![shape(32, 4, 1)],
            passband: 0.5,
            taps,
            stopband_edge: stop_edge,
            min_stopband_db: atten,
            max_ripple_db: 0.1,
            method: Method::Remez,
        }
    }

    /// **Optimality, established rather than compared.**
    ///
    /// Chebyshev's alternation theorem: a length-`2M+1` linear-phase
    /// filter whose weighted error reaches its maximum magnitude at
    /// `M+2` points with alternating signs *is* the best such filter.
    /// So this test does not check that Remez beat something — it checks
    /// the condition that makes the answer optimal, which needs no
    /// reference implementation to compare against.
    ///
    /// # Why the tolerance is not zero
    ///
    /// The exchange is exactly equiripple **on its design grid**: at
    /// convergence every grid extremum has magnitude
    /// [`Design::delta`], and the loop's stopping condition is that no
    /// grid point exceeds it. That part is asserted exactly.
    ///
    /// Between grid points the continuous error overshoots slightly,
    /// because the weighted error curves sharply near the band edge
    /// where `1/H` is largest. At the textbook 32 points per extremum
    /// that overshoot was 12%; at 256 it is about 3%. This is inherent
    /// to grid-based Parks-McClellan rather than a defect here — the
    /// alternative is local refinement of each extremum, which is a
    /// larger piece of machinery for a few percent.
    ///
    /// So: alternation and count are exact; magnitude equality is
    /// asserted to the grid's resolution, and the figure is stated
    /// rather than tuned until it passed.
    #[test]
    fn the_alternation_condition_holds() {
        for taps in [15usize, 21, 31] {
            let s = spec(taps, 0.7, 50.0);
            let d = design(s.clone()).expect("must design");
            assert!(
                d.converged,
                "taps {taps}: the exchange must converge for this spec"
            );
            let errs = remez::alternation(&s).expect("extrema");
            let m = taps / 2;
            assert!(
                errs.len() >= m + 2,
                "taps {taps}: expected at least {} extrema, found {}",
                m + 2,
                errs.len()
            );
            // Signs must alternate -- exactly, no tolerance.
            for w in errs.windows(2) {
                assert!(
                    (w[0] > 0.0) != (w[1] > 0.0),
                    "taps {taps}: extrema do not alternate: {errs:?}"
                );
            }
            // Magnitudes equal to the grid's resolution.
            let mags: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
            let hi = mags.iter().cloned().fold(0.0f64, f64::max);
            let lo = mags.iter().cloned().fold(f64::INFINITY, f64::min);
            println!(
                "taps {taps}: delta {:.4e}, continuous {:.4e}..{:.4e} ({:.1}% spread)",
                d.delta,
                lo,
                hi,
                100.0 * (hi - lo) / hi
            );
            assert!(
                hi - lo <= 0.05 * hi,
                "taps {taps}: spread {:.1}% exceeds the grid's resolution",
                100.0 * (hi - lo) / hi
            );
            // And the on-grid minimax must sit inside that range: the
            // continuous error can overshoot delta but never undershoot
            // every extremum, which would mean the exchange solved a
            // different problem.
            assert!(
                d.delta <= hi * 1.001 && d.delta >= lo * 0.999,
                "taps {taps}: delta {} outside the measured range {lo}..{hi}",
                d.delta
            );
        }
    }

    /// **The point of the exercise.** For a stopband requirement, Remez
    /// must beat least squares on worst-case attenuation at equal taps.
    #[test]
    fn remez_beats_least_squares_on_the_stopband() {
        for taps in [21usize, 31] {
            let ls = design(Spec {
                method: Method::LeastSquares,
                ..spec(taps, 0.7, 50.0)
            })
            .unwrap();
            let rz = design(spec(taps, 0.7, 50.0)).unwrap();
            println!(
                "taps {taps}: least squares {:.1} dB / {:.4} dB ripple, \
                 remez {:.1} dB / {:.4} dB ripple",
                ls.stopband_db, ls.ripple_db, rz.stopband_db, rz.ripple_db
            );
            assert!(
                rz.stopband_db > ls.stopband_db,
                "taps {taps}: remez {} should beat least squares {}",
                rz.stopband_db,
                ls.stopband_db
            );
        }
    }

    /// Remez needs no weight search: the two dB targets fix it.
    #[test]
    fn the_weight_is_analytic() {
        let s = spec(31, 0.7, 50.0);
        let d = design(s.clone()).unwrap();
        // delta_target * 10^(As/20), with delta_target = Rp / 17.372.
        let expected = (s.max_ripple_db / 17.372) * 10f64.powf(s.min_stopband_db / 20.0);
        assert!(
            (d.stopband_weight - expected).abs() < 1e-12,
            "{} vs {expected}",
            d.stopband_weight
        );
    }

    /// A deeper stopband is bought with taps, monotonically.
    #[test]
    fn deeper_stopbands_cost_taps() {
        let mut prev = 0.0;
        for taps in [11usize, 17, 25, 35] {
            let d = design(spec(taps, 0.7, 60.0)).unwrap();
            if !d.converged {
                continue;
            }
            println!("taps {taps}: {:.1} dB", d.stopband_db);
            assert!(
                d.stopband_db >= prev - 1.0,
                "more taps must not reject less: {} then {}",
                prev,
                d.stopband_db
            );
            prev = d.stopband_db;
        }
    }

    /// Cascade targets work here too, and each stage at its own rate.
    #[test]
    fn it_compensates_a_cascade() {
        let s = Spec {
            cics: vec![shape(8, 2, 1), shape(61, 5, 1)],
            passband: 0.5,
            taps: 21,
            stopband_edge: 0.8,
            min_stopband_db: 40.0,
            max_ripple_db: 0.1,
            method: Method::Remez,
        };
        let d = design(s).expect("must design");
        println!(
            "cascade: ripple {:.4} dB, stopband {:.1} dB, converged {}",
            d.ripple_db, d.stopband_db, d.converged
        );
        assert!(d.taps.len() == 21);
        // Symmetric, so linear phase -- the property the whole shape
        // exists for.
        for k in 0..21 {
            assert!((d.taps[k] - d.taps[20 - k]).abs() < 1e-12);
        }
    }

    /// Failure is reported, not disguised.
    #[test]
    fn an_even_tap_count_is_rejected() {
        assert!(design(spec(16, 0.7, 40.0)).is_none());
    }
}
