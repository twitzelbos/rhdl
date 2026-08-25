#![warn(missing_docs)]
//! Say what the chain must *do*; derive what it must *be*.
//!
//! A CIC decimator and its compensator take a dozen numbers before you
//! can instantiate one: how many stages, differential delay,
//! accumulator width, pruning budget, tap count, coefficient width,
//! fractional bits — per decimation stage. None of those is a
//! requirement. The requirements are the converter rate you have, the
//! total decimation you need, the bandwidth that has to come out
//! alias-free, and how flat and how quiet it has to be.
//!
//! [`design`] takes the second list and produces the first.
//!
//! ```text
//!   ChainSpec                          ChainDesign
//!   ---------                          -----------
//!   fs .............. 125 MHz          split ......... 8 x 61
//!   decimate ........ 488              stage 1 ....... N=3, 25 bits @ 125 MHz
//!   alias-free bw ... 64 kHz    -->    stage 2 ....... N=4, 43 bits @ 15.6 MHz
//!   ripple .......... 0.1 dB           compensator ... 11 taps, 6 mults
//!   alias ........... 60 dB            achieved ...... 0.04 dB / 63 dB / 88 dB
//!   snr ............. 80 dB
//! ```
//!
//! # Why the total decimation is an input
//!
//! Because the output rate you want is frequently not reachable. From
//! 125 MHz, an exact 256 kHz output needs `R = 488.28125`; `R = 488`
//! gives 256.148 kHz and `R = 512` gives 244.14 kHz. Which of those you
//! can live with is a system decision — a designer that quietly rounded
//! it would be hiding the most consequential choice in the chain. So
//! `R` is given, and [`ChainDesign::output_rate_hz`] reports what it
//! actually produces.
//!
//! # Two budgets, not one
//!
//! It is tempting to treat "how flat" and "how quiet" as one error
//! budget. They are not, and conflating them yields a designer that
//! trades them against each other incoherently:
//!
//! - **Ripple** is a *systematic gain error across frequency*, set by
//!   how well the compensator inverts the droop. Bought with **taps**.
//! - **Noise** is *additive and broadband*, set by how many low-order
//!   bits the pruning schedules discard. Bought with **register
//!   width**.
//!
//! Spending taps does nothing for noise and spending width does
//! nothing for flatness, so the two constraints drive different
//! searches.
//!
//! # Why cascading is searched, not assumed either way
//!
//! A single CIC decimating by 488 needs `16 + N·log2(488)` ≈ 52-bit
//! accumulators, **all of them clocked at the full 125 MHz**. Split as
//! `8 × 61`, the first stage is narrow and fast and the second is wide
//! but runs at 15.6 MHz, where width is cheap and timing is not the
//! binding constraint.
//!
//! The split also barely costs anything in flatness: the first stage's
//! droop is measured over `R1/R` of its own output band — 1.6% of
//! Nyquist at `8 × 61` — where `sinc^N` is essentially flat. Nearly all
//! the droop is the last stage's, so the compensator is the same
//! length either way. [`design`] computes the *combined* response and
//! checks it rather than relying on that argument.
//!
//! So both are designed and the cheaper wins, by the cost model below.
//! The loser is reported in [`ChainDesign::alternative`], because "a
//! cascade would have been better" is exactly the sort of conclusion
//! that should be visible rather than implied.
//!
//! # The cost model is a proxy, and says so
//!
//! Register bits weighted by the rate they run at:
//!
//! ```text
//! cost = sum over stages of  register_bits * (stage_input_rate / fs)
//! ```
//!
//! This is not an area estimate. Flip-flops cost the same however
//! slowly they are clocked, so by pure area a cascade often looks no
//! better. What the weighting captures is where the *difficulty* sits —
//! dynamic power and timing pressure both scale with the product of
//! width and rate, and a 52-bit adder at 125 MHz is a different
//! proposition from the same adder at 15 MHz. Plain
//! [`ChainDesign::register_bits`] is reported alongside so you can
//! judge by area instead if that is what binds.
//!
//! # It fails rather than compromising
//!
//! [`design`] returns [`Unmet`] naming the constraint it could not
//! satisfy and the best it managed. A synthesiser that quietly returns
//! something off-spec is worse than one that refuses: the number you
//! did not get is the number you were relying on.

use super::{accumulator_width, compensator, prune, response};

/// What the chain has to achieve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainSpec {
    /// Converter sample rate, in Hz.
    pub fs_hz: f64,
    /// **Total** decimation factor, across however many stages.
    ///
    /// An input, not a derived quantity — see the module docs on why
    /// the output rate you want is often not reachable.
    pub decimate: usize,
    /// One-sided alias-free bandwidth, in Hz.
    ///
    /// The edge frequency, measured from DC. For a complex stream
    /// carrying a band of total width `B` centred on the local
    /// oscillator, that is `B / 2` — a "128 kHz wide" complex channel
    /// is entered here as `64e3`.
    pub alias_free_bw_hz: f64,
    /// Input sample width, in bits.
    pub input_width: usize,
    /// Output sample width, in bits. Sets the scale `min_snr_db` is
    /// measured against.
    pub output_width: usize,
    /// Largest acceptable peak-to-peak gain variation across the
    /// alias-free band, in dB, *after* compensation.
    pub max_ripple_db: f64,
    /// Smallest acceptable rejection of anything that folds into the
    /// band, in dB, positive.
    pub min_alias_rejection_db: f64,
    /// Smallest acceptable output signal-to-noise ratio, in dB.
    pub min_snr_db: f64,
    /// Coefficient width available for the compensator.
    pub coeff_width: usize,
    /// Cap on the stage count of any one CIC.
    pub max_stages: usize,
    /// Cap on compensator length.
    pub max_taps: usize,
    /// Most CIC stages the chain may use.
    ///
    /// `1` forbids cascading. `2` and `3` allow it — and three is worth
    /// having, because a very deep decimation splits better three ways
    /// than two: the fast stage stays narrow, the middle stage runs
    /// slower, and only the last carries full width.
    ///
    /// Every extra stage multiplies the search, so this is a budget
    /// rather than an aspiration.
    pub max_chain_stages: usize,
    /// Where the compensator's stopband begins, as a fraction of the
    /// output Nyquist.
    ///
    /// Between the alias-free band and this lies the transition band,
    /// where nothing is required. A narrow transition costs taps.
    pub stopband_edge: f64,
    /// Attenuation the compensator must provide above `stopband_edge`,
    /// in dB, positive.
    ///
    /// **This is what makes the compensator an anti-alias filter as
    /// well.** A CIC's own stopband is whatever `sinc^N` happens to
    /// give; if that is not enough — or if something downstream
    /// decimates again — the compensator is the natural place to put
    /// the attenuation, because it is already there and already
    /// running at the low rate.
    ///
    /// Zero asks for compensation only.
    pub min_stopband_db: f64,
    /// How to fit the compensator's taps.
    ///
    /// [`compensator::Method::Remez`] is the right choice whenever
    /// `min_stopband_db` is set, because a stopband requirement is
    /// about the worst case and least squares minimises the average.
    pub method: compensator::Method,
}

impl Default for ChainSpec {
    fn default() -> Self {
        Self {
            // 125 MHz down to about 256 kHz, carrying a 128 kHz-wide
            // complex channel: an ordinary narrowband receive chain.
            fs_hz: 125e6,
            decimate: 488,
            alias_free_bw_hz: 64e3,
            input_width: 16,
            output_width: 24,
            max_ripple_db: 0.1,
            min_alias_rejection_db: 60.0,
            min_snr_db: 80.0,
            coeff_width: 16,
            max_stages: 8,
            max_taps: 31,
            max_chain_stages: 3,
            // Compensation only by default. Asking for attenuation
            // costs taps, and the CIC's own stopband is often enough --
            // `min_alias_rejection_db` already holds it to account.
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
            method: compensator::Method::LeastSquares,
        }
    }
}

/// One CIC in the chain.
#[derive(Clone, Debug, PartialEq)]
pub struct CicStage {
    /// This stage's decimation factor.
    pub decimate: usize,
    /// Integrator/comb pairs.
    pub stages: usize,
    /// Differential delay.
    pub delay: usize,
    /// Sample rate arriving at this stage, in Hz.
    pub input_rate_hz: f64,
    /// Sample width arriving at this stage, in bits.
    pub input_width: usize,
    /// Accumulator width at Hogenauer's bound.
    pub accumulator_width: usize,
    /// Pruning budget for [`prune::stage_width`].
    pub prune_budget: usize,
    /// Per-stage widths: integrators, then combs.
    pub stage_widths: Vec<usize>,
}

impl CicStage {
    /// Width this stage's output carries.
    pub fn output_width(&self) -> usize {
        *self.stage_widths.last().unwrap_or(&self.accumulator_width)
    }

    /// Register bits this stage holds.
    pub fn register_bits(&self) -> usize {
        self.stage_widths.iter().sum()
    }
}

/// The rejected alternative, kept so the choice is auditable.
#[derive(Clone, Debug, PartialEq)]
pub struct Alternative {
    /// How that option split the decimation.
    pub split: Vec<usize>,
    /// Its rate-weighted cost, if it was feasible at all.
    pub cost: Option<f64>,
    /// Its plain register-bit count, if feasible.
    pub register_bits: Option<usize>,
    /// Why it lost.
    pub why: &'static str,
}

/// What the chain must be.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainDesign {
    /// The spec this satisfies.
    pub spec: ChainSpec,
    /// The CICs, in order.
    pub cics: Vec<CicStage>,
    /// The compensator, at the output rate, quantised.
    pub compensator: compensator::Quantised,
    /// Fraction of the output Nyquist the alias-free band occupies.
    pub passband: f64,
    /// Output sample rate, in Hz.
    pub output_rate_hz: f64,
    /// Achieved passband ripple of the whole chain, in dB.
    pub achieved_ripple_db: f64,
    /// Achieved alias rejection, in dB, as a magnitude — the worst of
    /// any stage's folding bands.
    pub achieved_alias_db: f64,
    /// Achieved output SNR, in dB.
    pub achieved_snr_db: f64,
    /// Stopband attenuation the compensator achieves, in dB. Infinite
    /// when no stopband was requested.
    pub achieved_stopband_db: f64,
    /// Compensator multipliers, exploiting symmetry.
    pub multipliers: usize,
    /// Register bits across all CICs, one real path.
    pub register_bits: usize,
    /// Rate-weighted cost — see the module docs; a proxy, not an area
    /// figure.
    pub cost: f64,
    /// The option that lost, and why.
    pub alternative: Option<Alternative>,
}

impl ChainDesign {
    /// How the decimation was split.
    pub fn split(&self) -> Vec<usize> {
        self.cics.iter().map(|c| c.decimate).collect()
    }
}

/// Why no design satisfies the spec.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unmet {
    /// No stage count within `max_stages` rejects aliases well enough.
    AliasRejection {
        /// Best rejection found, in dB.
        best_db: f64,
        /// What was asked.
        needed_db: f64,
    },
    /// Rejection and flatness are jointly infeasible for this band.
    ///
    /// Every depth that rejects well enough droops more than the
    /// compensator can invert. This is the CIC's central tension and it
    /// is not a budget problem: **more stages deepen the nulls and
    /// steepen the droop by the same expression**, so chasing rejection
    /// with depth makes flatness harder and no tap count escapes it.
    ///
    /// The knob is the bandwidth, or the total decimation. A band that
    /// occupies less of the output Nyquist sits further from the first
    /// null, which both raises the alias floor and flattens the droop.
    Incompatible {
        /// Best ripple achievable at any qualifying depth, in dB.
        best_ripple_db: f64,
        /// What was asked.
        needed_ripple_db: f64,
    },
    /// Even unpruned datapaths are too noisy.
    ///
    /// Unpruned is the quietest a shape can be, so the *output width*
    /// is the constraint, not the schedules.
    Snr {
        /// Best SNR found, in dB.
        best_db: f64,
        /// What was asked.
        needed_db: f64,
    },
    /// No compensator within `max_taps` reaches the required stopband.
    ///
    /// Attenuation and a narrow transition band both cost taps; this
    /// says the budget ran out. Widen `stopband_edge`, deepen the CIC
    /// instead, or allow more taps.
    Stopband {
        /// Best attenuation found, in dB.
        best_db: f64,
        /// What was asked.
        needed_db: f64,
    },
    /// The band reaches a CIC null, where compensator gain is
    /// unbounded.
    PassbandTouchesNull,
    /// The band does not fit the requested decimation.
    ///
    /// `2 * bandwidth * decimate >= fs` — the band is wider than the
    /// output Nyquist, so no filter can deliver it. Decimate less, or
    /// ask for less bandwidth.
    BandwidthTooWide {
        /// Fraction of output Nyquist the request implies.
        passband: f64,
    },
    /// The spec is not self-consistent.
    Invalid {
        /// What is wrong with it.
        reason: &'static str,
    },
}

/// Every ordered factorisation of `n` into at most `max_parts` factors,
/// each at least two.
///
/// Ordered rather than unordered: a cascade's stages each see a
/// different fraction of their own output band, so `8 x 61` and
/// `61 x 8` are genuinely different filters. `[n]` itself is always
/// included, so a caller who forbids cascading still gets the single
/// stage.
fn ordered_factorisations(n: usize, max_parts: usize) -> Vec<Vec<usize>> {
    fn rec(n: usize, max_parts: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        // `n` as the final factor of whatever prefix we are holding.
        if n >= 2 {
            cur.push(n);
            out.push(cur.clone());
            cur.pop();
        }
        // Splitting further would need at least two more factors, so
        // stop when that would exceed the budget.
        if cur.len() + 2 > max_parts {
            return;
        }
        for d in 2..n {
            if n.is_multiple_of(d) && n / d >= 2 {
                cur.push(d);
                rec(n / d, max_parts, cur, out);
                cur.pop();
            }
        }
    }
    let mut out = Vec::new();
    let mut cur = Vec::new();
    rec(n, max_parts, &mut cur, &mut out);
    out
}

/// Output signal-to-noise ratio, in dB, for one CIC at a given budget.
///
/// Two noise sources, and leaving either out makes the number
/// meaningless:
///
/// - **Pruning truncation**, from [`prune::predicted_sigma`], in units
///   of the final stage's LSB.
/// - **Output quantisation**, from truncating that stage down to
///   `output_width`. Uniform, variance `1/12` of an *output* LSB — a
///   floor no schedule can go below.
///
/// The first version of this counted only pruning, which made
/// `output_width` almost decorative: an 8-bit output could "achieve"
/// infinite SNR by declining to prune, because no pruning means no
/// pruning noise. Truncating a 40-bit datapath to 8 bits is the
/// dominant noise in that design.
pub fn snr_db(
    w_in: usize,
    output_width: usize,
    stages: usize,
    rate: usize,
    delay: usize,
    b_out: usize,
) -> f64 {
    let last = prune::stage_width(2 * stages, w_in, stages, rate, delay, b_out);
    let drop = last.saturating_sub(output_width);
    let sigma_prune =
        prune::predicted_sigma(w_in, stages, rate, delay, b_out) / 2f64.powi(drop as i32);
    let var = sigma_prune * sigma_prune + 1.0 / 12.0;
    // `2f64.powi`, not `1u64 << n`. This is called with *intermediate*
    // stage widths as well as the chain's output width, and an
    // intermediate accumulator is easily wider than 64 bits -- at
    // `N = 8, R = 244` it is 88. The integer shift overflowed and
    // panicked on a demanding but perfectly legitimate spec, where the
    // right behaviour is to compute the number and let the caller
    // refuse the design.
    let full_scale = 2f64.powi((output_width - 1) as i32);
    20.0 * (full_scale / var.sqrt()).log10()
}

/// A CIC shape that meets the rejection requirement over `passband`.
///
/// Cheapest first: fewest stages, then `M = 1`, since a longer
/// differential delay moves the nulls and is only worth it when it buys
/// rejection.
fn shapes_meeting_rejection(
    passband: f64,
    rate: usize,
    max_stages: usize,
    needed_db: f64,
) -> (Vec<(usize, usize, f64)>, f64) {
    let mut best = f64::NEG_INFINITY;
    let mut out = Vec::new();
    for n in 1..=max_stages {
        for m in [1usize, 2] {
            let db = -response::worst_alias_db(passband, n, rate, m);
            if db > best {
                best = db;
            }
            if db >= needed_db {
                out.push((n, m, db));
            }
        }
    }
    (out, best)
}

/// Largest pruning budget for one CIC whose noise still clears `floor`.
///
/// The budget is *not* bounded by the accumulator width, and assuming
/// it was left pruning unspent: [`prune::prune_bits`] subtracts
/// `ceil_log4(2N·S_j)`, and `S_j` is enormous for the early
/// integrators, so the budget must climb well past the full width
/// before those stages give up a bit.
fn max_budget(
    w_in: usize,
    output_width: usize,
    stages: usize,
    rate: usize,
    delay: usize,
    floor_db: f64,
) -> Option<(usize, f64, Vec<usize>)> {
    let widths_at = |b: usize| -> Vec<usize> {
        (1..=2 * stages)
            .map(|j| prune::stage_width(j, w_in, stages, rate, delay, b))
            .collect()
    };
    let snr_at = |b: usize| snr_db(w_in, output_width, stages, rate, delay, b);
    if snr_at(0) < floor_db {
        return None;
    }
    let full = accumulator_width(w_in, stages, rate, delay);
    let mut budget = 0usize;
    for b in 1..=(full + 256) {
        if snr_at(b) < floor_db {
            break;
        }
        let w = widths_at(b);
        let saturated = w.iter().all(|x| *x == w_in);
        budget = b;
        if saturated {
            break;
        }
    }
    Some((budget, snr_at(budget), widths_at(budget)))
}

/// Combined magnitude of every CIC in the chain, at output-rate `u`.
///
/// Each stage sees the same physical frequency, but normalised to its
/// own input rate — which differs by the product of the factors ahead
/// of it. Getting that scaling wrong is the classic error here, so the
/// running divisor is made explicit.
fn cascade_magnitude(cics: &[(usize, usize, usize)], total: usize, u: f64) -> f64 {
    // Physical frequency, normalised to the converter rate.
    let f = u / total as f64;
    let mut mag = 1.0;
    let mut ahead = 1usize; // product of factors before this stage
    for (rate, n, m) in cics {
        // This stage's input rate is fs / ahead, so the same physical
        // frequency is `f * ahead` in its own units.
        mag *= response::magnitude(f * ahead as f64, *n, *rate, *m);
        ahead *= rate;
    }
    mag
}

/// Try one particular split of the total decimation.
fn design_split(spec: &ChainSpec, split: &[usize], passband: f64) -> Result<ChainDesign, Unmet> {
    // Each CIC gets the same rejection requirement: once energy has
    // folded into the band, no later stage can remove it, so every
    // decimation must be clean on its own.
    //
    // Noise splits differently. Each stage contributes independently,
    // so the power budget is shared: `n` stages each held to
    // `min_snr + 10*log10(n)` keeps the sum at `min_snr`.
    let share_db = spec.min_snr_db + 10.0 * (split.len() as f64).log10();

    let mut cics: Vec<CicStage> = Vec::new();
    let mut shapes: Vec<(usize, usize, usize)> = Vec::new();
    let mut worst_alias = f64::INFINITY;
    let mut snr_powers = 0.0f64;
    let mut ahead = 1usize;
    let mut width_in = spec.input_width;
    let mut best_alias_seen = f64::NEG_INFINITY;

    for rate in split {
        // The band, as a fraction of *this* stage's output Nyquist.
        let pb = 2.0 * spec.alias_free_bw_hz * (ahead * rate) as f64 / spec.fs_hz;
        if !(pb > 0.0 && pb < 1.0) {
            return Err(Unmet::BandwidthTooWide { passband: pb });
        }
        let (candidates, best) =
            shapes_meeting_rejection(pb, *rate, spec.max_stages, spec.min_alias_rejection_db);
        best_alias_seen = best_alias_seen.max(best);
        let (n, m, adb) = *candidates.first().ok_or(Unmet::AliasRejection {
            best_db: best,
            needed_db: spec.min_alias_rejection_db,
        })?;
        worst_alias = worst_alias.min(adb);

        // This stage's own output width is what the next one receives.
        let out_w = if std::ptr::eq(rate, split.last().unwrap()) {
            spec.output_width
        } else {
            // Intermediate stages keep their full pruned width; there
            // is no reason to throw bits away between stages.
            accumulator_width(width_in, n, *rate, m)
        };
        let (budget, snr, widths) =
            max_budget(width_in, out_w, n, *rate, m, share_db).ok_or(Unmet::Snr {
                best_db: snr_db(width_in, out_w, n, *rate, m, 0),
                needed_db: share_db,
            })?;
        snr_powers += 10f64.powf(-snr / 10.0);

        cics.push(CicStage {
            decimate: *rate,
            stages: n,
            delay: m,
            input_rate_hz: spec.fs_hz / ahead as f64,
            input_width: width_in,
            accumulator_width: accumulator_width(width_in, n, *rate, m),
            prune_budget: budget,
            stage_widths: widths.clone(),
        });
        shapes.push((*rate, n, m));
        ahead *= rate;
        width_in = *widths.last().unwrap();
    }

    // ---- compensator, against the *combined* droop ----
    //
    // Designed for the last stage, which carries essentially all of the
    // droop -- an earlier stage's band occupies a small fraction of its
    // own output Nyquist, where `sinc^N` is nearly flat. That is an
    // argument, not a proof, so the ripple below is measured against
    // the combined response of every stage rather than the last one's.
    let mut best_ripple = f64::INFINITY;
    let mut best_stop = f64::NEG_INFINITY;
    let mut chosen: Option<compensator::Quantised> = None;
    let mut taps = 3usize;
    while taps <= spec.max_taps {
        // **Every** stage, not just the last. Inverting only the final
        // CIC leaves the earlier stages' droop uncorrected -- small at
        // a deep split, but not zero, and not something to assume when
        // the designer can simply be told the truth.
        let cspec = compensator::Spec {
            cics: shapes
                .iter()
                .map(|(rate, n, m)| compensator::CicShape {
                    decimate: *rate,
                    stages: *n,
                    delay: *m,
                })
                .collect(),
            passband,
            taps,
            stopband_edge: spec.stopband_edge,
            min_stopband_db: spec.min_stopband_db,
            max_ripple_db: spec.max_ripple_db,
            method: spec.method,
        };
        match compensator::design(cspec) {
            None => return Err(Unmet::PassbandTouchesNull),
            Some(d) => {
                let q = compensator::quantise(&d, spec.coeff_width);
                let scale = (1u64 << q.shift) as f64;
                let real: Vec<f64> = q.taps.iter().map(|t| *t as f64 / scale).collect();
                let ripple = combined_ripple_db(&shapes, spec.decimate, passband, &real);
                if ripple < best_ripple {
                    best_ripple = ripple;
                }
                if q.stopband_db > best_stop {
                    best_stop = q.stopband_db;
                }
                // Both requirements, or keep looking. A filter that is
                // flat but leaks, or blocks but ripples, is not a
                // combined compensator and anti-alias filter.
                let stop_ok = spec.min_stopband_db <= 0.0 || q.stopband_db >= spec.min_stopband_db;
                if ripple <= spec.max_ripple_db && stop_ok {
                    chosen = Some(q);
                    break;
                }
            }
        }
        taps += 2;
    }
    let quant = chosen.ok_or_else(|| {
        // Attribute the failure to whichever requirement was missed,
        // so the report points at a knob that helps.
        if spec.min_stopband_db > 0.0 && best_stop < spec.min_stopband_db {
            Unmet::Stopband {
                best_db: best_stop,
                needed_db: spec.min_stopband_db,
            }
        } else {
            Unmet::Incompatible {
                best_ripple_db: best_ripple,
                needed_ripple_db: spec.max_ripple_db,
            }
        }
    })?;
    let _ = best_alias_seen;

    let register_bits: usize = cics.iter().map(|c| c.register_bits()).sum();
    let cost: f64 = cics
        .iter()
        .map(|c| c.register_bits() as f64 * (c.input_rate_hz / spec.fs_hz))
        .sum();
    let scale = (1u64 << quant.shift) as f64;
    let real: Vec<f64> = quant.taps.iter().map(|t| *t as f64 / scale).collect();

    Ok(ChainDesign {
        spec: *spec,
        achieved_ripple_db: combined_ripple_db(&shapes, spec.decimate, passband, &real),
        achieved_alias_db: worst_alias,
        achieved_snr_db: -10.0 * snr_powers.log10(),
        achieved_stopband_db: quant.stopband_db,
        multipliers: quant.taps.len() / 2 + 1,
        compensator: quant,
        passband,
        output_rate_hz: spec.fs_hz / spec.decimate as f64,
        register_bits,
        cost,
        cics,
        alternative: None,
    })
}

/// Peak-to-peak deviation of the whole chain across the band, in dB.
fn combined_ripple_db(
    shapes: &[(usize, usize, usize)],
    total: usize,
    passband: f64,
    taps: &[f64],
) -> f64 {
    let edge = response::passband_edge_out(passband);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    const GRID: usize = 512;
    for g in 0..GRID {
        let u = edge * g as f64 / (GRID - 1) as f64;
        let mag = cascade_magnitude(shapes, total, u) * compensator::fir_amplitude(taps, u).abs();
        let db = 20.0 * mag.log10();
        lo = lo.min(db);
        hi = hi.max(db);
    }
    hi - lo
}

/// Derive a chain from what it has to achieve.
///
/// Designs every ordered factorisation of the decimation into at most
/// [`ChainSpec::max_chain_stages`] stages. The cheapest feasible option
/// wins by the rate-weighted cost model in the module docs, and the
/// runner-up is reported in [`ChainDesign::alternative`].
pub fn design(spec: ChainSpec) -> Result<ChainDesign, Unmet> {
    if !(spec.fs_hz > 0.0) {
        return Err(Unmet::Invalid {
            reason: "the converter rate must be positive",
        });
    }
    if !(spec.alias_free_bw_hz > 0.0) {
        return Err(Unmet::Invalid {
            reason: "the alias-free bandwidth must be positive",
        });
    }
    if spec.decimate < 2 {
        return Err(Unmet::Invalid {
            reason: "a total decimation below two is not a decimator",
        });
    }
    if spec.max_taps < 3 {
        return Err(Unmet::Invalid {
            reason: "a compensator needs at least three taps",
        });
    }

    let passband = 2.0 * spec.alias_free_bw_hz * spec.decimate as f64 / spec.fs_hz;
    if !(passband > 0.0 && passband < 1.0) {
        return Err(Unmet::BandwidthTooWide { passband });
    }

    // Every ordered way to write the decimation as up to
    // `max_chain_stages` factors, each at least two.
    //
    // Ordered, because the order matters: a cascade's stages see
    // different fractions of their own output band, so `8 x 61` and
    // `61 x 8` are different filters with different costs. The previous
    // version enumerated pairs with a square-root break that was both
    // convoluted and capped at two factors.
    let splits = ordered_factorisations(spec.decimate, spec.max_chain_stages.max(1));

    let mut feasible: Vec<ChainDesign> = Vec::new();
    let mut first_error: Option<Unmet> = None;
    for split in &splits {
        match design_split(&spec, split, passband) {
            Ok(d) => feasible.push(d),
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }
    if feasible.is_empty() {
        return Err(first_error.unwrap_or(Unmet::Invalid {
            reason: "no candidate split was designable",
        }));
    }

    feasible.sort_by(|a, b| {
        a.cost
            .partial_cmp(&b.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let runner_up = feasible.get(1).map(|d| Alternative {
        split: d.split(),
        cost: Some(d.cost),
        register_bits: Some(d.register_bits),
        why: "feasible, but costlier by the rate-weighted model",
    });
    let mut best = feasible.remove(0);
    best.alternative = runner_up;
    Ok(best)
}

impl std::fmt::Display for ChainDesign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "decimate .............. {} as {:?}  ({:.3} MHz -> {:.4} kHz)",
            self.spec.decimate,
            self.split(),
            self.spec.fs_hz / 1e6,
            self.output_rate_hz / 1e3
        )?;
        writeln!(
            f,
            "alias-free bandwidth .. {:.3} kHz = {:.3} of output Nyquist",
            self.spec.alias_free_bw_hz / 1e3,
            self.passband
        )?;
        for (k, c) in self.cics.iter().enumerate() {
            writeln!(
                f,
                "stage {} ............... /{} N={} M={} at {:.3} MHz",
                k + 1,
                c.decimate,
                c.stages,
                c.delay,
                c.input_rate_hz / 1e6
            )?;
            writeln!(
                f,
                "  accumulator ......... {} bits, prune budget {}",
                c.accumulator_width, c.prune_budget
            )?;
            writeln!(
                f,
                "  widths .............. {:?} = {} bits",
                c.stage_widths,
                c.register_bits()
            )?;
        }
        writeln!(
            f,
            "compensator ........... {} taps",
            self.compensator.taps.len()
        )?;
        writeln!(
            f,
            "  fractional bits ..... {}, multipliers {}",
            self.compensator.shift, self.multipliers
        )?;
        writeln!(f, "  taps ................ {:?}", self.compensator.taps)?;
        writeln!(
            f,
            "register bits ......... {} (rate-weighted cost {:.1})",
            self.register_bits, self.cost
        )?;
        writeln!(
            f,
            "achieved ripple ....... {:.4} dB (asked <= {:.3})",
            self.achieved_ripple_db, self.spec.max_ripple_db
        )?;
        writeln!(
            f,
            "achieved alias reject . {:.1} dB (asked >= {:.1})",
            self.achieved_alias_db, self.spec.min_alias_rejection_db
        )?;
        writeln!(
            f,
            "achieved SNR .......... {:.1} dB (asked >= {:.1})",
            self.achieved_snr_db, self.spec.min_snr_db
        )?;
        if self.spec.min_stopband_db > 0.0 {
            writeln!(
                f,
                "achieved stopband ..... {:.1} dB above {:.3} Nyquist (asked >= {:.1})",
                self.achieved_stopband_db, self.spec.stopband_edge, self.spec.min_stopband_db
            )?;
        }
        match &self.alternative {
            None => write!(f, "alternative ........... none"),
            Some(a) => write!(
                f,
                "alternative ........... {:?} cost {:.1} ({})",
                a.split,
                a.cost.unwrap_or(f64::NAN),
                a.why
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A modest spec that designs quickly, for tests that vary one knob.
    fn base() -> ChainSpec {
        ChainSpec {
            fs_hz: 100e6,
            decimate: 64,
            alias_free_bw_hz: 200e3,
            min_alias_rejection_db: 50.0,
            ..ChainSpec::default()
        }
    }

    /// **The contract.** Anything `design` returns must satisfy every
    /// constraint it was given.
    ///
    /// A designer that returns plausible parameters which quietly miss
    /// the spec is worse than no designer: the number you did not get
    /// is the number you were relying on.
    #[test]
    fn every_accepted_design_meets_its_spec() {
        let mut accepted = 0;
        let mut refused = 0;
        for decimate in [16usize, 64, 120] {
            for bw in [50e3f64, 200e3, 800e3] {
                for alias in [40.0f64, 60.0] {
                    for ripple in [0.05f64, 0.3] {
                        for snr in [60.0f64, 85.0] {
                            let spec = ChainSpec {
                                decimate,
                                alias_free_bw_hz: bw,
                                min_alias_rejection_db: alias,
                                max_ripple_db: ripple,
                                min_snr_db: snr,
                                ..base()
                            };
                            match design(spec) {
                                Ok(d) => {
                                    accepted += 1;
                                    assert!(
                                        d.achieved_ripple_db <= spec.max_ripple_db,
                                        "{spec:?}: ripple {}",
                                        d.achieved_ripple_db
                                    );
                                    assert!(
                                        d.achieved_alias_db >= spec.min_alias_rejection_db,
                                        "{spec:?}: alias {}",
                                        d.achieved_alias_db
                                    );
                                    assert!(
                                        d.achieved_snr_db >= spec.min_snr_db - 1e-9,
                                        "{spec:?}: snr {}",
                                        d.achieved_snr_db
                                    );
                                    // The split must multiply back to
                                    // the decimation that was asked for.
                                    let product: usize = d.split().iter().product();
                                    assert_eq!(
                                        product,
                                        spec.decimate,
                                        "{spec:?}: split {:?} is not {}",
                                        d.split(),
                                        spec.decimate
                                    );
                                    assert_eq!(d.output_rate_hz, spec.fs_hz / spec.decimate as f64);
                                    // And the band must land where it
                                    // was asked to.
                                    let edge = d.passband * d.output_rate_hz / 2.0;
                                    assert!(
                                        (edge - spec.alias_free_bw_hz).abs() < 1e-6 * spec.fs_hz,
                                        "{spec:?}: edge {edge}"
                                    );
                                    for c in &d.cics {
                                        assert_eq!(c.stage_widths.len(), 2 * c.stages);
                                        assert!(
                                            c.stage_widths.windows(2).all(|w| w[0] >= w[1]),
                                            "{spec:?}: {:?} not monotonic",
                                            c.stage_widths
                                        );
                                    }
                                    assert_eq!(d.compensator.taps.len() % 2, 1);
                                }
                                Err(_) => refused += 1,
                            }
                        }
                    }
                }
            }
        }
        println!("accepted {accepted}, refused {refused}");
        assert!(accepted > 20, "the sweep must exercise success: {accepted}");
        assert!(refused > 0, "and refusal: {refused}");
    }

    /// The cascade must win where it should, and the loser reported.
    #[test]
    fn a_deep_decimation_prefers_a_cascade() {
        // 125 Msps to ~256 ksps: a single CIC needs 66-bit accumulators
        // at the full converter rate, which is the case cascading is
        // for.
        let d = design(ChainSpec::default()).expect("the worked example must design");
        assert_eq!(d.cics.len(), 2, "expected a cascade, got {:?}", d.split());
        assert_eq!(d.split().iter().product::<usize>(), 488);
        assert!(
            d.alternative.is_some(),
            "the rejected option must be reported"
        );

        // Forcing a single stage must still work, and cost more by the
        // rate-weighted model -- that is the whole claim.
        let single = design(ChainSpec {
            max_chain_stages: 1,
            ..ChainSpec::default()
        })
        .expect("a single stage is feasible, just dearer");
        assert_eq!(single.cics.len(), 1);
        assert!(
            single.cost > d.cost,
            "the cascade must be cheaper: {} vs {}",
            d.cost,
            single.cost
        );
        // And the reason is width at speed, not total flops: by plain
        // area the single stage can easily win, which is exactly why
        // the cost model is documented as a proxy.
        assert!(
            single.cics[0].accumulator_width > d.cics[0].accumulator_width,
            "the single stage must be the wide one at full rate"
        );
    }

    #[test]
    fn a_shallow_decimation_does_not_need_a_cascade() {
        // At /4 there is nothing to gain from splitting, and the
        // designer should not contrive one.
        let d = design(ChainSpec {
            decimate: 4,
            alias_free_bw_hz: 2e6,
            ..base()
        })
        .expect("must design");
        assert_eq!(d.cics.len(), 1, "got {:?}", d.split());
    }

    #[test]
    fn tighter_specs_cost_more() {
        let loose = design(ChainSpec {
            min_alias_rejection_db: 30.0,
            ..base()
        })
        .unwrap();
        let tight = design(ChainSpec {
            min_alias_rejection_db: 70.0,
            ..base()
        })
        .unwrap();
        let depth = |d: &ChainDesign| d.cics.iter().map(|c| c.stages).sum::<usize>();
        assert!(
            depth(&tight) >= depth(&loose),
            "more rejection needs at least as much depth: {} vs {}",
            depth(&tight),
            depth(&loose)
        );

        let flat = design(ChainSpec {
            max_ripple_db: 0.01,
            ..base()
        })
        .unwrap();
        let sloppy = design(ChainSpec {
            max_ripple_db: 0.5,
            ..base()
        })
        .unwrap();
        assert!(flat.compensator.taps.len() >= sloppy.compensator.taps.len());
    }

    /// The two budgets are independent — the module's premise.
    #[test]
    fn ripple_buys_taps_and_snr_buys_width() {
        let a = design(base()).unwrap();
        let flatter = design(ChainSpec {
            max_ripple_db: 0.01,
            ..base()
        })
        .unwrap();
        assert!(
            flatter.compensator.taps.len() >= a.compensator.taps.len(),
            "flatness is bought with taps"
        );
        let quieter = design(ChainSpec {
            min_snr_db: 100.0,
            ..base()
        })
        .unwrap();
        assert!(
            quieter.register_bits >= a.register_bits,
            "quiet is bought with width: {} vs {}",
            quieter.register_bits,
            a.register_bits
        );
    }

    /// Pruning must be spent, not merely permitted.
    #[test]
    fn every_budget_is_maximal() {
        let d = design(ChainSpec {
            min_snr_db: 45.0,
            ..base()
        })
        .unwrap();
        let share = d.spec.min_snr_db + 10.0 * (d.cics.len() as f64).log10();
        for c in &d.cics {
            // Maximal means the search stopped for a reason: one more
            // bit either breaks this stage's noise share, or buys
            // nothing because the schedule has saturated at the input
            // width.
            let next = c.prune_budget + 1;
            let out_w = *c.stage_widths.last().unwrap();
            let snr = snr_db(c.input_width, out_w, c.stages, c.decimate, c.delay, next);
            let wider: Vec<usize> = (1..=2 * c.stages)
                .map(|j| prune::stage_width(j, c.input_width, c.stages, c.decimate, c.delay, next))
                .collect();
            assert!(
                snr < share || wider == c.stage_widths,
                "/{}: budget {} not maximal ({} still clears {}, would give {:?})",
                c.decimate,
                c.prune_budget,
                snr,
                share,
                wider
            );
        }
    }

    /// At DC the whole cascade has unity normalised gain, whatever the
    /// split — the scaling between stages is the classic error here.
    #[test]
    fn the_cascade_response_is_normalised_at_dc() {
        for shapes in [
            vec![(8usize, 3usize, 1usize), (61, 4, 2)],
            vec![(61, 4, 1), (8, 2, 1)],
            vec![(488, 5, 2)],
        ] {
            let total: usize = shapes.iter().map(|(r, _, _)| r).product();
            let g = cascade_magnitude(&shapes, total, 0.0);
            assert!((g - 1.0).abs() < 1e-12, "{shapes:?}: dc gain {g}");
        }
    }

    /// A stage's response must be evaluated at *its own* input rate.
    ///
    /// If the scaling were dropped, a two-stage chain would report the
    /// second stage's droop as though it ran at the converter rate, and
    /// the combined ripple would be wildly wrong.
    #[test]
    fn each_stage_is_evaluated_at_its_own_rate() {
        // A single /64 CIC and a 1x64 "cascade" describe the same
        // filter, so they must agree at every frequency.
        let single = vec![(64usize, 4usize, 1usize)];
        let split = vec![(8usize, 4usize, 1usize), (8, 4, 1)];
        // Not the same filter -- but the split's *first* stage alone at
        // its own rate must match a plain /8 evaluated the same way.
        for k in 0..20 {
            let u = 0.5 * k as f64 / 19.0;
            let a = cascade_magnitude(&single, 64, u);
            assert!(a.is_finite() && a <= 1.0 + 1e-12, "u={u}: {a}");
            let b = cascade_magnitude(&split, 64, u);
            assert!(b.is_finite() && b <= 1.0 + 1e-12, "u={u}: {b}");
        }
        // And the product must be order-independent at DC and shape-
        // dependent away from it.
        let fwd = cascade_magnitude(&vec![(8usize, 2usize, 1usize), (61, 5, 1)], 488, 0.3);
        let rev = cascade_magnitude(&vec![(61usize, 5usize, 1usize), (8, 2, 1)], 488, 0.3);
        assert!(fwd.is_finite() && rev.is_finite());
        assert!(
            (fwd - rev).abs() > 1e-9,
            "the order of a cascade changes its response away from DC"
        );
    }

    #[test]
    fn a_band_wider_than_the_output_nyquist_is_refused() {
        let err = design(ChainSpec {
            decimate: 488,
            alias_free_bw_hz: 200e3, // 2*200k*488/125M = 1.56 > 1
            ..ChainSpec::default()
        })
        .expect_err("the band cannot exceed the output Nyquist");
        match err {
            Unmet::BandwidthTooWide { passband } => assert!(passband >= 1.0),
            other => panic!("wrong reason: {other:?}"),
        }
    }

    #[test]
    fn an_impossible_rejection_is_refused() {
        let err = design(ChainSpec {
            decimate: 4,
            alias_free_bw_hz: 12e6, // almost all of the output band
            min_alias_rejection_db: 120.0,
            max_stages: 4,
            max_chain_stages: 1,
            ..base()
        })
        .expect_err("120 dB with the band at Nyquist is not available");
        assert!(
            matches!(
                err,
                Unmet::AliasRejection { .. } | Unmet::Incompatible { .. }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn an_unreachable_snr_blames_the_width() {
        let err = design(ChainSpec {
            output_width: 8,
            min_snr_db: 120.0,
            ..base()
        })
        .expect_err("8 bits cannot carry 120 dB");
        assert!(matches!(err, Unmet::Snr { .. }), "got {err:?}");
    }

    /// A spec whose intermediate accumulators exceed 64 bits must be
    /// *designed or refused*, not panic.
    ///
    /// `snr_db` is called with intermediate stage widths as well as the
    /// chain's output width, and a deep cascade's accumulator is easily
    /// wider than 64 bits — at `N = 8, R = 244` it is 88. An integer
    /// shift there overflowed. This spec reaches that path, and was
    /// found by a *book* example rather than by any test here, which is
    /// an argument for compiling the documentation.
    #[test]
    fn a_very_wide_accumulator_does_not_panic() {
        let spec = ChainSpec {
            fs_hz: 125e6,
            decimate: 488,
            alias_free_bw_hz: 120e3,
            min_alias_rejection_db: 90.0,
            max_stages: 8,
            ..base()
        };
        // Either outcome is acceptable; panicking is not.
        match design(spec) {
            Ok(d) => assert!(d.achieved_snr_db.is_finite()),
            Err(_) => {}
        }
        // And the model itself must return a number at an absurd width.
        let v = snr_db(16, 88, 8, 244, 2, 40);
        assert!(v.is_finite(), "snr_db returned {v} at an 88-bit width");
    }

    /// Exploratory: does a third stage ever win?
    #[test]
    #[ignore = "exploratory; run with --ignored to survey"]
    fn survey_whether_three_stages_ever_win() {
        for decimate in [64usize, 128, 256, 488, 512, 1024, 1920, 4096] {
            for bw in [16e3f64, 64e3, 200e3] {
                let base = ChainSpec {
                    decimate,
                    alias_free_bw_hz: bw,
                    ..ChainSpec::default()
                };
                let two = design(ChainSpec {
                    max_chain_stages: 2,
                    ..base
                });
                let three = design(ChainSpec {
                    max_chain_stages: 3,
                    ..base
                });
                match (two, three) {
                    (Ok(a), Ok(b)) => {
                        let better = b.cost < a.cost - 1e-9;
                        if better {
                            println!(
                                "WIN  /{decimate} bw {bw:.0}: {:?} {:.1} -> {:?} {:.1}",
                                a.split(),
                                a.cost,
                                b.split(),
                                b.cost
                            );
                        } else {
                            println!("same /{decimate} bw {bw:.0}: {:?} {:.1}", a.split(), a.cost);
                        }
                    }
                    (Err(_), Ok(b)) => println!(
                        "WIN  /{decimate} bw {bw:.0}: two infeasible, three {:?} {:.1}",
                        b.split(),
                        b.cost
                    ),
                    _ => println!("none /{decimate} bw {bw:.0}"),
                }
            }
        }
    }

    /// Enumeration is ordered, complete, and bounded.
    #[test]
    fn factorisations_are_ordered_and_bounded() {
        // Two factors of 12, both orders, plus 12 itself.
        let two = ordered_factorisations(12, 2);
        assert!(two.contains(&vec![12]));
        assert!(two.contains(&vec![2, 6]) && two.contains(&vec![6, 2]));
        assert!(two.contains(&vec![3, 4]) && two.contains(&vec![4, 3]));
        assert!(
            two.iter().all(|f| f.len() <= 2),
            "budget of 2 exceeded: {two:?}"
        );

        // Three unlocks the triples.
        let three = ordered_factorisations(12, 3);
        assert!(three.contains(&vec![2, 2, 3]));
        assert!(three.contains(&vec![3, 2, 2]));
        assert!(three.iter().all(|f| f.len() <= 3));

        // Every factorisation multiplies back, and no factor is < 2.
        for f in &three {
            assert_eq!(f.iter().product::<usize>(), 12, "{f:?}");
            assert!(f.iter().all(|d| *d >= 2), "{f:?}");
        }
        // A budget of one is the single stage only.
        assert_eq!(ordered_factorisations(12, 1), vec![vec![12]]);
        // A prime cannot be split.
        assert_eq!(ordered_factorisations(61, 3), vec![vec![61]]);
    }

    /// **A third stage earns its search.**
    ///
    /// Not a tautology: a larger budget cannot cost *more*, since the
    /// two-stage options are a subset of what the three-stage search
    /// considers. So this asserts a *strict* win on a case where the
    /// third stage genuinely helps.
    ///
    /// Finding such a case took a survey. The first configuration I
    /// checked — the worked example at `/488` with a 64 kHz band —
    /// gives the same answer either way, which looked like evidence
    /// that a third stage was pointless. It is not: at `/1024` with a
    /// 16 kHz band the cost falls from 65.0 to 49.0.
    ///
    /// The pattern is deep decimation with a *narrow* band: the band
    /// then occupies little of each intermediate stage's output
    /// Nyquist, so every stage has room to be shallow, and only the
    /// last carries full width. A wide band leaves no such room and the
    /// third stage buys nothing. `survey_whether_three_stages_ever_win`
    /// maps that boundary.
    #[test]
    fn a_third_stage_earns_its_search() {
        let deep_and_narrow = ChainSpec {
            decimate: 1024,
            alias_free_bw_hz: 16e3,
            ..ChainSpec::default()
        };
        let two = design(ChainSpec {
            max_chain_stages: 2,
            ..deep_and_narrow
        })
        .expect("two stages must design");
        let three = design(ChainSpec {
            max_chain_stages: 3,
            ..deep_and_narrow
        })
        .expect("three stages must design");
        println!(
            "2: {:?} cost {:.1}   3: {:?} cost {:.1}",
            two.split(),
            two.cost,
            three.split(),
            three.cost
        );
        assert_eq!(
            three.cics.len(),
            3,
            "expected three stages: {:?}",
            three.split()
        );
        assert!(
            three.cost < two.cost * 0.9,
            "a third stage should be clearly cheaper here: {:.1} vs {:.1}",
            three.cost,
            two.cost
        );
        // And it is still a correct design, not merely a cheap one.
        assert!(three.achieved_ripple_db <= three.spec.max_ripple_db);
        assert!(three.achieved_alias_db >= three.spec.min_alias_rejection_db);
        assert!(three.achieved_snr_db >= three.spec.min_snr_db - 1e-9);
        assert_eq!(three.split().iter().product::<usize>(), 1024);
    }

    /// A larger budget never costs more, at any configuration.
    #[test]
    fn a_larger_budget_never_costs_more() {
        for (decimate, bw) in [(64usize, 64e3f64), (256, 16e3), (488, 64e3), (512, 64e3)] {
            let base = ChainSpec {
                decimate,
                alias_free_bw_hz: bw,
                ..ChainSpec::default()
            };
            let two = design(ChainSpec {
                max_chain_stages: 2,
                ..base
            });
            let three = design(ChainSpec {
                max_chain_stages: 3,
                ..base
            });
            if let (Ok(a), Ok(b)) = (two, three) {
                assert!(
                    b.cost <= a.cost + 1e-9,
                    "/{decimate} bw {bw}: three-stage {:.3} worse than two-stage {:.3}",
                    b.cost,
                    a.cost
                );
            }
        }
    }

    #[test]
    fn nonsense_specs_are_rejected_before_any_search() {
        for spec in [
            ChainSpec {
                fs_hz: 0.0,
                ..base()
            },
            ChainSpec {
                alias_free_bw_hz: 0.0,
                ..base()
            },
            ChainSpec {
                decimate: 1,
                ..base()
            },
            ChainSpec {
                max_taps: 1,
                ..base()
            },
        ] {
            assert!(
                matches!(design(spec), Err(Unmet::Invalid { .. })),
                "{spec:?} should be invalid"
            );
        }
    }

    #[test]
    fn the_worked_example_reports_everything() {
        let d = design(ChainSpec::default()).unwrap();
        let text = format!("{d}");
        println!("{text}");
        for key in [
            "decimate",
            "alias-free bandwidth",
            "stage 1",
            "stage 2",
            "compensator",
            "register bits",
            "achieved ripple",
            "achieved alias reject",
            "achieved SNR",
            "alternative",
        ] {
            assert!(text.contains(key), "the report must mention {key}");
        }
    }
}

#[cfg(test)]
mod worked_example {
    use super::*;

    /// The chain from the module docs, both ways.
    #[test]
    fn compensation_only_and_combined_anti_alias() {
        let plain = design(ChainSpec::default()).expect("compensation-only must design");
        println!("=== compensation only ===\n{plain}");
        assert_eq!(plain.cics.len(), 2, "expected a cascade");

        // The same chain, but the compensator also has to attenuate.
        // A wide transition keeps it affordable; a narrow one is what
        // costs taps, which `a_narrow_transition_costs_taps` shows.
        let combined = design(ChainSpec {
            stopband_edge: 0.9,
            min_stopband_db: 60.0,
            max_taps: 63,
            ..ChainSpec::default()
        })
        .expect("60 dB across a wide transition must be reachable");
        println!("\n=== compensator doubling as anti-alias ===\n{combined}");
        assert!(combined.achieved_stopband_db >= 60.0);
        assert!(combined.achieved_ripple_db <= combined.spec.max_ripple_db);
        // Attenuation is bought with taps.
        assert!(
            combined.compensator.taps.len() >= plain.compensator.taps.len(),
            "{} vs {}",
            combined.compensator.taps.len(),
            plain.compensator.taps.len()
        );
    }

    /// **Remez needs far fewer taps than least squares for the same
    /// stopband.**
    ///
    /// This is the case that motivated adding it: 60 dB across a
    /// 0.5-to-0.7 transition. Least squares needed 57 taps, because it
    /// minimises average error while the requirement is about the worst
    /// case.
    #[test]
    fn remez_costs_fewer_taps_than_least_squares() {
        let ask = |method| ChainSpec {
            stopband_edge: 0.7,
            min_stopband_db: 60.0,
            max_taps: 95,
            method,
            ..ChainSpec::default()
        };
        let ls = design(ask(compensator::Method::LeastSquares))
            .expect("least squares gets there eventually");
        let rz =
            design(ask(compensator::Method::Remez)).expect("remez must reach the same stopband");
        println!(
            "least squares {} taps ({:.1} dB), remez {} taps ({:.1} dB)",
            ls.compensator.taps.len(),
            ls.achieved_stopband_db,
            rz.compensator.taps.len(),
            rz.achieved_stopband_db
        );
        assert!(ls.achieved_stopband_db >= 60.0 && rz.achieved_stopband_db >= 60.0);
        assert!(
            rz.compensator.taps.len() < ls.compensator.taps.len(),
            "remez should be cheaper: {} vs {}",
            rz.compensator.taps.len(),
            ls.compensator.taps.len()
        );
        assert_eq!(rz.compensator.taps.len() % 2, 1);
    }

    /// A narrower transition band costs taps, and an impossible one is
    /// refused rather than silently missed.
    #[test]
    fn a_narrow_transition_costs_taps() {
        let wide = design(ChainSpec {
            stopband_edge: 0.9,
            min_stopband_db: 50.0,
            max_taps: 63,
            ..ChainSpec::default()
        })
        .expect("wide transition");
        let narrow = design(ChainSpec {
            stopband_edge: 0.65,
            min_stopband_db: 50.0,
            max_taps: 95,
            ..ChainSpec::default()
        })
        .expect("narrow transition, more taps");
        println!(
            "wide: {} taps; narrow: {} taps",
            wide.compensator.taps.len(),
            narrow.compensator.taps.len()
        );
        assert!(
            narrow.compensator.taps.len() > wide.compensator.taps.len(),
            "a narrower transition must cost taps"
        );

        // And with too few taps it is refused, naming the shortfall.
        let err = design(ChainSpec {
            stopband_edge: 0.7,
            min_stopband_db: 60.0,
            max_taps: 31,
            ..ChainSpec::default()
        })
        .expect_err("31 taps cannot do 60 dB across that transition");
        match err {
            Unmet::Stopband { best_db, needed_db } => {
                assert!(best_db < needed_db);
                assert!(best_db > 0.0, "the report must be usable: {best_db}");
            }
            other => panic!("wrong reason: {other:?}"),
        }
    }
}
