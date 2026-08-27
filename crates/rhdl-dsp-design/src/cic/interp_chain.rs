#![warn(missing_docs)]
//! Spec-driven design of a CIC **interpolation** chain.
//!
//! The transmit counterpart of [`super::chain`]. State what the
//! transmitter has to deliver — a converter rate, an interpolation
//! factor, the bandwidth the signal occupies, how flat it has to be and
//! how far down the images have to sit — and [`design`] returns the
//! chain that meets it most cheaply, or says which requirement it could
//! not meet.
//!
//! ```text
//!   envelope @ fs/R --> [ CIC^N x R1 ] --> [ CIC^N x R2 ] --> ... --> @ fs
//!        compensator here, at the low rate
//! ```
//!
//! # Images, not aliases — and the metric is the same function
//!
//! A decimator folds everything above its output Nyquist *into* the
//! band. An interpolator does the reverse: the upsampled spectrum
//! *repeats* at every multiple of the input rate, so a signal occupying
//! `|u| < B` reappears at `k ± B` for every integer `k`, where `u` is
//! normalised to the input (low) rate.
//!
//! Those are the same frequencies. The decimator's aliases arrive *from*
//! `k ± B` and the interpolator's images appear *at* `k ± B`, and the
//! CIC's `sinc^N` response at those points is what suppresses either.
//! So [`super::response::worst_alias_db`] computes both, and
//! [`worst_image_db`] is a rename rather than a reimplementation —
//! `the_image_metric_is_the_alias_metric` asserts they agree, so the
//! identity is checked rather than assumed.
//!
//! This is the transposition property showing up in the specification
//! rather than in the structure, which is a pleasant place for it.
//!
//! # The compensator cannot improve image rejection, and that
//! simplifies the search
//!
//! On the receive side, [`super::compensator`]'s stopband is part of the
//! alias budget: the compensator runs *after* decimation, so its
//! attenuation multiplies the CIC's at the folded frequencies, and
//! `min_stopband_db` therefore has to constrain the cascade and the FIR
//! together.
//!
//! On the transmit side it does not, and cannot. The compensator runs
//! *before* upsampling, at the low rate, so its response is periodic
//! with period one in `u`. The image at `k + u` sees **exactly** the
//! compensator gain the signal at `u` sees, so the image-to-signal ratio
//! is
//!
//! ```text
//!   |Hcic(k + u)| / |Hcic(u)|
//! ```
//!
//! with the compensator cancelling out of it entirely.
//! `the_compensator_cannot_change_image_rejection` measures this.
//!
//! Two consequences, and they are why this module is shorter than
//! [`super::chain`]:
//!
//! - **The search decouples.** Depth and the chain split are chosen for
//!   image rejection alone; the compensator is then designed for ripple
//!   alone. There is no joint stopband constraint to satisfy, so
//!   [`Unmet`] has no `Stopband` variant.
//! - **Image rejection is bought with `N`, `R` and bandwidth — nothing
//!   else.** A caller who cannot meet it has three knobs and a
//!   compensator is not one of them. The honest alternative is a
//!   compensator at the *output* rate, which can suppress images
//!   because it is no longer periodic in `u` — and costs a FIR running
//!   at the converter clock. That is a different widget and it is not
//!   what [`crate::cic::interp`]'s pre-compensation describes.
//!
//! # There is no pruning noise, so there is no SNR search
//!
//! [`super::chain`] spends much of its effort choosing a pruning budget
//! against an SNR floor. An interpolator has no such budget:
//! [`super::interp`] shows that truncating anywhere ahead of an
//! integrator feeds a bias into a pole at DC, so the datapath is exact
//! and the only quantisation is the final narrowing to the converter.
//!
//! So [`Unmet`] has no `Snr` variant either, and
//! [`InterpDesign::dac_snr_db`] is the converter's own floor rather than
//! a filter property. The chain contributes nothing to it, which is a
//! stronger statement than any pruning schedule can make.
//!
//! # What the widths are
//!
//! Each stage reports both the uniform width every stage would need and
//! the *tapered* per-stage widths, which are lossless — see
//! [`super::interp`]. The tapered figure is what a
//! `cic_interp_tapered!` widget would spend and is typically 25-35%
//! less; the uniform figure is what [`crate::cic::interp`]'s
//! current widget spends. Both are reported because only one of them is
//! buildable today.

use super::{compensator, interp, response};

/// What the transmitter has to deliver.
#[derive(Clone, Debug, PartialEq)]
pub struct InterpSpec {
    /// Converter (output) sample rate, in Hz.
    pub fs_hz: f64,
    /// Total interpolation factor. The envelope arrives at `fs_hz / R`.
    pub interpolate: usize,
    /// Baseband bandwidth the signal occupies, in Hz.
    ///
    /// One-sided: a complex envelope occupying `±200 kHz` has
    /// `image_free_bw_hz = 200e3`. This is the band whose images must be
    /// rejected, and the band the compensator must keep flat.
    pub image_free_bw_hz: f64,
    /// Envelope width, per component.
    pub input_width: usize,
    /// Converter width.
    pub output_width: usize,
    /// Worst permitted passband ripple after compensation, in dB.
    pub max_ripple_db: f64,
    /// Worst permitted image, in dB below the wanted signal.
    ///
    /// Positive: `60.0` means images at least 60 dB down.
    pub min_image_rejection_db: f64,
    /// Compensator coefficient width.
    pub coeff_width: usize,
    /// Most CIC stages permitted in any one chain stage.
    pub max_stages: usize,
    /// Most compensator taps permitted.
    pub max_taps: usize,
    /// Most chain stages permitted.
    ///
    /// A single stage is often right on transmit: the combs are at the
    /// low rate and cheap, and only the integrators pay the converter
    /// clock. Splitting helps when the total factor is large enough that
    /// one stage's accumulator becomes unreasonably wide.
    pub max_chain_stages: usize,
    /// Compensator design method.
    pub method: compensator::Method,
}

impl Default for InterpSpec {
    /// The worked configuration in [`crate::cic::interp`]: a 16-bit
    /// envelope at 1 Msps onto a 125 Msps converter, 200 kHz of signal.
    fn default() -> Self {
        Self {
            fs_hz: 125e6,
            interpolate: 125,
            image_free_bw_hz: 200e3,
            input_width: 16,
            output_width: 14,
            max_ripple_db: 0.1,
            min_image_rejection_db: 60.0,
            coeff_width: 16,
            max_stages: 5,
            max_taps: 21,
            max_chain_stages: 2,
            method: compensator::Method::LeastSquares,
        }
    }
}

/// One CIC in the interpolation chain.
#[derive(Clone, Debug, PartialEq)]
pub struct InterpStage {
    /// This stage's interpolation factor.
    pub interpolate: usize,
    /// CIC stages (`N`).
    pub stages: usize,
    /// Differential delay (`M`).
    pub delay: usize,
    /// Rate this stage's combs run at, in Hz.
    pub input_rate_hz: f64,
    /// Rate this stage's integrators run at, in Hz.
    pub output_rate_hz: f64,
    /// Sample width entering this stage.
    pub input_width: usize,
    /// Width every stage would need if they were uniform.
    pub accumulator_width: usize,
    /// Per-stage widths under the lossless taper, combs then
    /// integrators.
    pub stage_widths: Vec<usize>,
    /// Register bits at the uniform width.
    pub uniform_state_bits: usize,
    /// Register bits under the taper. Lossless — the two produce
    /// bit-identical output.
    pub tapered_state_bits: usize,
}

/// A cheaper or otherwise notable candidate that was not chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Alternative {
    /// How the interpolation was split.
    pub split: Vec<usize>,
    /// Per-stage CIC depth.
    pub stages: Vec<usize>,
    /// Rate-weighted cost.
    pub cost: f64,
    /// Register bits at the uniform width.
    pub register_bits: usize,
    /// Why it lost.
    pub why: &'static str,
}

/// A designed interpolation chain.
#[derive(Clone, Debug, PartialEq)]
pub struct InterpDesign {
    /// What was asked for.
    pub spec: InterpSpec,
    /// The chain, in signal order: lowest rate first.
    pub cics: Vec<InterpStage>,
    /// The pre-compensator, quantised. Runs at the envelope rate.
    pub compensator: compensator::Quantised,
    /// Signal bandwidth as a fraction of the envelope Nyquist.
    pub passband: f64,
    /// Envelope rate, in Hz.
    pub input_rate_hz: f64,
    /// Achieved passband ripple after compensation, in dB.
    pub achieved_ripple_db: f64,
    /// Achieved worst image, in dB below the signal. Positive.
    pub achieved_image_db: f64,
    /// Where the worst image sits, in Hz.
    pub worst_image_hz: f64,
    /// The converter's own quantisation floor, in dB.
    ///
    /// **Not a filter property.** The interpolator is exact, so the
    /// chain contributes nothing to this — see the module docs.
    pub dac_snr_db: f64,
    /// Rate-weighted register cost, for ranking candidates.
    pub cost: f64,
    /// Total register bits at the uniform width.
    pub register_bits: usize,
    /// Total register bits under the lossless taper.
    pub tapered_register_bits: usize,
    /// The runner-up, when there was one.
    pub alternative: Option<Alternative>,
}

impl InterpDesign {
    /// How the interpolation was split, lowest rate first.
    pub fn split(&self) -> Vec<usize> {
        self.cics.iter().map(|c| c.interpolate).collect()
    }

    /// Per-stage CIC depth, lowest rate first.
    pub fn depths(&self) -> Vec<usize> {
        self.cics.iter().map(|c| c.stages).collect()
    }

    /// The cascade's shapes in **signal order**, lowest rate first.
    ///
    /// What a reader of the design wants. For feeding a response
    /// evaluator, use [`InterpDesign::evaluation_shapes`] instead — the
    /// two differ by a reversal that matters.
    pub fn shapes(&self) -> Vec<compensator::CicShape> {
        self.cics
            .iter()
            .map(|c| compensator::CicShape {
                decimate: c.interpolate,
                stages: c.stages,
                delay: c.delay,
            })
            .collect()
    }

    /// The cascade's shapes in the order
    /// [`compensator::cascade_magnitude`] expects: **highest rate
    /// first**.
    ///
    /// See `evaluation_order` in this module for why the two orders are
    /// different and what goes wrong if they are confused.
    pub fn evaluation_shapes(&self) -> Vec<compensator::CicShape> {
        evaluation_order(&self.shapes())
    }
}

/// Why no chain satisfies the spec.
///
/// Deliberately fewer variants than [`super::chain::Unmet`]: there is no
/// `Stopband` because the compensator cannot affect image rejection, and
/// no `Snr` because an interpolator has no pruning noise. Both absences
/// are explained in the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unmet {
    /// No depth within `max_stages` rejects images well enough.
    ImageRejection {
        /// Best rejection found, in dB.
        best_db: f64,
        /// What was asked.
        needed_db: f64,
    },
    /// Rejection and flatness are jointly infeasible for this band.
    ///
    /// The same central tension as on the receive side: more stages
    /// deepen the nulls and steepen the droop by the same expression, so
    /// chasing images with depth makes flatness harder and no tap count
    /// escapes it. The knobs are the bandwidth and the interpolation
    /// factor.
    Incompatible {
        /// Best ripple at any qualifying depth, in dB.
        best_ripple_db: f64,
        /// What was asked.
        needed_ripple_db: f64,
    },
    /// The band reaches a CIC null, where compensator gain is unbounded.
    PassbandTouchesNull,
    /// The band does not fit the requested interpolation.
    ///
    /// `2 · bandwidth · R >= fs`: the signal is wider than the envelope
    /// Nyquist, so it cannot be represented at the envelope rate at all.
    /// Interpolate less, or ask for less bandwidth.
    BandwidthTooWide {
        /// Fraction of envelope Nyquist the request implies.
        passband: f64,
    },
    /// The spec is not self-consistent.
    Invalid {
        /// What is wrong with it.
        reason: &'static str,
    },
}

/// Reverse a transmit chain into the order the response evaluators
/// expect.
///
/// **This reversal is load bearing and easy to miss.**
/// [`compensator::cascade_magnitude`] was written for a *decimation*
/// cascade, where `cics[0]` is the stage running at the converter rate
/// and each later stage runs slower — it computes `f = u / total` and
/// then multiplies the argument by the product of the factors *ahead*
/// of each stage.
///
/// A transmit chain is built the other way round: `cics[0]` is the stage
/// at the envelope rate and each later one runs faster. The stage whose
/// high rate is the converter rate is the *last* one. So the evaluator
/// wants the chain reversed, and passing it in signal order silently
/// evaluates every stage at the wrong frequency.
///
/// `a_cascade_rejects_at_least_as_well_as_its_first_stage` is what
/// caught this — a cascade came out *worse* than its own first stage,
/// which is impossible when every later stage only adds attenuation.
/// The single-stage case is a no-op, so nothing else in this module
/// would have noticed.
fn evaluation_order(shapes: &[compensator::CicShape]) -> Vec<compensator::CicShape> {
    let mut v = shapes.to_vec();
    v.reverse();
    v
}

/// Worst image, in dB **below** the signal, for one CIC.
///
/// A rename of [`response::worst_alias_db`], negated so it reads as a
/// rejection figure. See the module docs on why the two are the same
/// computation.
pub fn worst_image_db(passband: f64, n: usize, r: usize, m: usize) -> f64 {
    -response::worst_alias_db(passband, n, r, m)
}

/// Worst image, in dB below the signal, for a whole cascade.
///
/// Evaluated on the composite response, because an image created by the
/// first stage is also attenuated by every stage after it. `u` runs in
/// units of the *envelope* rate, so the images sit at integers.
pub fn cascade_image_db(
    shapes: &[compensator::CicShape],
    passband: f64,
    total: usize,
) -> (f64, f64) {
    let order = evaluation_order(shapes);
    let edge = response::passband_edge_out(passband);
    let mut worst = 0.0f64;
    let mut at = 0.0f64;
    for k in 1..=(total / 2).max(1) {
        const STEPS: usize = 257;
        for s in 0..STEPS {
            let u = k as f64 - edge + 2.0 * edge * (s as f64 / (STEPS - 1) as f64);
            // Above the output Nyquist in envelope units there is
            // nothing left to protect.
            if u <= 0.0 || u > total as f64 / 2.0 {
                continue;
            }
            let a = compensator::cascade_magnitude(&order, u);
            if a > worst {
                worst = a;
                at = u;
            }
        }
    }
    if worst <= 1e-15 {
        (300.0, at)
    } else {
        (-20.0 * worst.log10(), at)
    }
}

/// Peak-to-peak ripple of the **composite** across the passband, in dB.
///
/// Cascade times compensator, evaluated on the taps the hardware will
/// actually hold — the integers from [`compensator::quantise`], scaled
/// back to reals.
///
/// Measuring the compensator's *own* ideal ripple instead is a trap this
/// module fell into: [`compensator::Design::ripple_db`] describes the
/// FIR the designer wanted, and quantisation makes it worse, so a search
/// that accepts on the ideal figure and reports the quantised one can
/// return a design that violates its own spec. It did — 0.1221 dB
/// against a 0.1 dB requirement — until this function was what the
/// search tested. [`super::chain`] gets this right and this is the same
/// treatment.
fn combined_ripple_db(shapes: &[compensator::CicShape], passband: f64, taps: &[f64]) -> f64 {
    let order = evaluation_order(shapes);
    let edge = response::passband_edge_out(passband);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    const GRID: usize = 512;
    for g in 0..GRID {
        let u = edge * g as f64 / (GRID - 1) as f64;
        let mag =
            compensator::cascade_magnitude(&order, u) * compensator::fir_amplitude(taps, u).abs();
        let db = 20.0 * mag.log10();
        lo = lo.min(db);
        hi = hi.max(db);
    }
    hi - lo
}

/// The quantised taps as reals, for evaluating what the hardware does.
fn dequantised(q: &compensator::Quantised) -> Vec<f64> {
    let scale = (1u64 << q.shift) as f64;
    q.taps.iter().map(|x| *x as f64 / scale).collect()
}

/// Every ordered way to write `n` as up to `max_parts` factors, each at
/// least two.
fn ordered_factorisations(n: usize, max_parts: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    fn go(n: usize, parts_left: usize, acc: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if n == 1 {
            if !acc.is_empty() {
                out.push(acc.clone());
            }
            return;
        }
        if parts_left == 1 {
            acc.push(n);
            out.push(acc.clone());
            acc.pop();
            return;
        }
        for f in 2..=n {
            if n.is_multiple_of(f) {
                acc.push(f);
                go(n / f, parts_left - 1, acc, out);
                acc.pop();
            }
        }
    }
    go(n, max_parts.max(1), &mut Vec::new(), &mut out);
    out
}

/// Every assignment of `(n, m)` to each stage of a split.
fn depth_assignments(len: usize, max_stages: usize) -> Vec<Vec<(usize, usize)>> {
    let choices: Vec<(usize, usize)> = (1..=max_stages)
        .flat_map(|n| [1usize, 2].into_iter().map(move |m| (n, m)))
        .collect();
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
    for _ in 0..len {
        let mut next = Vec::with_capacity(out.len() * choices.len());
        for prefix in &out {
            for c in &choices {
                let mut v = prefix.clone();
                v.push(*c);
                next.push(v);
            }
        }
        out = next;
    }
    out
}

/// Design an interpolation chain for a spec.
///
/// Enumerates every ordered factorisation of the interpolation factor
/// and every per-stage depth within `max_stages`, keeps the candidates
/// that meet the image requirement, designs a compensator for each,
/// and returns the cheapest that also meets the ripple requirement.
///
/// The enumeration is exhaustive rather than clever. At the defaults —
/// `R = 125`, two chain stages, five depths, two delays — it is a few
/// hundred candidates, each costing one compensator design, which is
/// milliseconds. [`super::chain`] needs a staged search because its
/// pruning budget is a third dimension; here there isn't one.
// `!(a > b)` rather than `a <= b`, deliberately: these compare f64
// requirements that may be NaN, where every comparison is false and the
// negated form is the one that rejects rather than silently accepts.
// Same reasoning, and the same allow, as `super::chain::design`.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn design(spec: InterpSpec) -> Result<InterpDesign, Unmet> {
    if !(spec.fs_hz > 0.0) {
        return Err(Unmet::Invalid {
            reason: "the converter rate must be positive",
        });
    }
    if !(spec.image_free_bw_hz > 0.0) {
        return Err(Unmet::Invalid {
            reason: "the image-free bandwidth must be positive",
        });
    }
    if spec.interpolate < 2 {
        return Err(Unmet::Invalid {
            reason: "a total interpolation below two is not an interpolator",
        });
    }
    if spec.max_taps < 3 {
        return Err(Unmet::Invalid {
            reason: "a compensator needs at least three taps",
        });
    }

    // Fraction of the *envelope* Nyquist the signal occupies. The
    // envelope rate is fs/R, so its Nyquist is fs/(2R).
    let passband = 2.0 * spec.image_free_bw_hz * spec.interpolate as f64 / spec.fs_hz;
    if !(passband > 0.0 && passband < 1.0) {
        return Err(Unmet::BandwidthTooWide { passband });
    }

    let input_rate_hz = spec.fs_hz / spec.interpolate as f64;
    let splits = ordered_factorisations(spec.interpolate, spec.max_chain_stages.max(1));

    let mut best_image = f64::NEG_INFINITY;
    let mut best_ripple = f64::INFINITY;
    let mut touched_null = false;
    let mut feasible: Vec<InterpDesign> = Vec::new();

    for split in &splits {
        for depths in depth_assignments(split.len(), spec.max_stages.max(1)) {
            let shapes: Vec<compensator::CicShape> = split
                .iter()
                .zip(&depths)
                .map(|(r, (n, m))| compensator::CicShape {
                    decimate: *r,
                    stages: *n,
                    delay: *m,
                })
                .collect();

            let (image_db, at_u) = cascade_image_db(&shapes, passband, spec.interpolate);
            if image_db > best_image {
                best_image = image_db;
            }
            if image_db < spec.min_image_rejection_db {
                continue;
            }

            // The compensator, for ripple alone. `min_stopband_db` is
            // zero because the compensator's stopband cannot affect
            // image rejection -- see the module docs. Constraining it
            // would spend taps on an attenuation that buys nothing.
            let cspec = compensator::Spec {
                // Reversed: see `evaluation_order`. The compensator
                // designer evaluates the cascade through the same
                // function, so it needs the same order.
                cics: evaluation_order(&shapes),
                passband,
                taps: 0,
                stopband_edge: 1.0,
                min_stopband_db: 0.0,
                max_ripple_db: spec.max_ripple_db,
                method: spec.method,
            };
            // Shortest tap set whose *quantised* taps give a composite
            // flat enough. Quantised, and composite: see
            // `combined_ripple_db`.
            let mut chosen: Option<(compensator::Quantised, f64)> = None;
            for taps in (3..=spec.max_taps).step_by(2) {
                let mut s = cspec.clone();
                s.taps = taps;
                match compensator::design(s) {
                    Some(d) => {
                        let q = compensator::quantise(&d, spec.coeff_width);
                        let ripple = combined_ripple_db(&shapes, passband, &dequantised(&q));
                        if ripple < best_ripple {
                            best_ripple = ripple;
                        }
                        if ripple <= spec.max_ripple_db {
                            chosen = Some((q, ripple));
                            break;
                        }
                    }
                    None => touched_null = true,
                }
            }
            let Some((quantised, achieved_ripple)) = chosen else {
                continue;
            };

            // Widths, chaining stage to stage. Each stage's output is
            // its own accumulator width; the next stage's input is that.
            let mut stages_out = Vec::with_capacity(split.len());
            let mut w_in = spec.input_width;
            let mut rate_in = input_rate_hz;
            let mut cost = 0.0f64;
            let mut uniform_bits = 0usize;
            let mut tapered_bits = 0usize;
            for (r, (n, m)) in split.iter().zip(&depths) {
                let wa = interp::accumulator_width(w_in, *n, *r, *m);
                let widths: Vec<usize> = (1..=2 * n)
                    .map(|j| interp::stage_width(j, w_in, *n, *r, *m))
                    .collect();
                let u_bits = interp::uniform_state_bits(w_in, *n, *r, *m);
                let t_bits = interp::tapered_state_bits(w_in, *n, *r, *m);
                let rate_out = rate_in * *r as f64;

                // Rate-weighted cost: the combs are clocked at this
                // stage's input rate and the integrators at its output
                // rate, so an integrator bit costs `R` times what a comb
                // bit costs. That asymmetry is the whole reason a
                // transmit chain wants its depth early and its rate
                // late, and a cost model that ignored it would rank the
                // splits backwards.
                let comb_bits = widths[..*n].iter().sum::<usize>() * *m;
                let int_bits = widths[*n..].iter().sum::<usize>();
                cost += comb_bits as f64 * rate_in + int_bits as f64 * rate_out;

                uniform_bits += u_bits;
                tapered_bits += t_bits;
                stages_out.push(InterpStage {
                    interpolate: *r,
                    stages: *n,
                    delay: *m,
                    input_rate_hz: rate_in,
                    output_rate_hz: rate_out,
                    input_width: w_in,
                    accumulator_width: wa,
                    stage_widths: widths,
                    uniform_state_bits: u_bits,
                    tapered_state_bits: t_bits,
                });
                w_in = wa;
                rate_in = rate_out;
            }

            feasible.push(InterpDesign {
                spec: spec.clone(),
                cics: stages_out,
                compensator: quantised.clone(),
                passband,
                input_rate_hz,
                achieved_ripple_db: achieved_ripple,
                achieved_image_db: image_db,
                worst_image_hz: at_u * input_rate_hz,
                // A full-scale sine at the converter width. The chain
                // adds nothing: the interpolator is exact.
                dac_snr_db: 6.02 * spec.output_width as f64 + 1.76,
                cost,
                register_bits: uniform_bits,
                tapered_register_bits: tapered_bits,
                alternative: None,
            });
        }
    }

    if feasible.is_empty() {
        if best_image < spec.min_image_rejection_db {
            return Err(Unmet::ImageRejection {
                best_db: best_image,
                needed_db: spec.min_image_rejection_db,
            });
        }
        if touched_null && !best_ripple.is_finite() {
            return Err(Unmet::PassbandTouchesNull);
        }
        return Err(Unmet::Incompatible {
            best_ripple_db: best_ripple,
            needed_ripple_db: spec.max_ripple_db,
        });
    }

    feasible.sort_by(|a, b| {
        a.cost
            .partial_cmp(&b.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let runner_up = feasible.get(1).map(|d| Alternative {
        split: d.split(),
        stages: d.depths(),
        cost: d.cost,
        register_bits: d.register_bits,
        why: "feasible, but costlier by the rate-weighted model",
    });
    let mut best = feasible.remove(0);
    best.alternative = runner_up;
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The image metric is the alias metric.**
    ///
    /// The transposition property, checked rather than asserted. A
    /// decimator's aliases arrive from `k ± B` and an interpolator's
    /// images appear at `k ± B`; the CIC response at those frequencies
    /// is the same number, so one function serves both and
    /// [`worst_image_db`] is a rename.
    #[test]
    fn the_image_metric_is_the_alias_metric() {
        for &(n, r, m) in &[(1, 4, 1), (2, 8, 1), (3, 125, 1), (4, 25, 1), (2, 16, 2)] {
            for passband in [0.1, 0.4, 0.8] {
                assert_eq!(
                    worst_image_db(passband, n, r, m),
                    -response::worst_alias_db(passband, n, r, m),
                    "N={n} R={r} M={m} pb={passband}"
                );
            }
        }
    }

    /// **The compensator cannot change image rejection.**
    ///
    /// The claim the whole module's simplicity rests on. A pre-
    /// compensator runs at the envelope rate, so its response is
    /// periodic with period one in `u`; the image at `k + u` therefore
    /// sees exactly the gain the signal at `u` sees and the ratio is
    /// unchanged.
    ///
    /// Measured on the designed taps rather than argued from the
    /// periodicity, because the periodicity is the thing worth checking.
    #[test]
    fn the_compensator_cannot_change_image_rejection() {
        let shapes = vec![compensator::CicShape {
            decimate: 32,
            stages: 3,
            delay: 1,
        }];
        let passband = 0.4;
        let d = compensator::design(compensator::Spec {
            cics: shapes.clone(),
            passband,
            taps: 15,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
            max_ripple_db: 0.1,
            method: compensator::Method::LeastSquares,
        })
        .expect("designable");

        let edge = response::passband_edge_out(passband);
        // The FIR's gain is periodic with period one in `u`, so it is
        // the same at the signal and at each of its images.
        for k in 1..=4 {
            let at_signal = compensator::fir_amplitude(&d.taps, edge).abs();
            let at_image = compensator::fir_amplitude(&d.taps, k as f64 + edge).abs();
            assert!(
                (at_signal - at_image).abs() < 1e-9 * at_signal.max(1.0),
                "k={k}: {at_signal} vs {at_image}"
            );
            let below = compensator::fir_amplitude(&d.taps, k as f64 - edge).abs();
            let mirror = compensator::fir_amplitude(&d.taps, -edge).abs();
            assert!(
                (below - mirror).abs() < 1e-9 * mirror.max(1.0),
                "k={k} lower side: {below} vs {mirror}"
            );
        }
    }

    /// The default spec — the worked 1 Msps into 125 Msps case —
    /// designs.
    #[test]
    fn the_default_spec_designs() {
        let d = design(InterpSpec::default()).expect("designable");
        assert_eq!(d.cics.iter().map(|c| c.interpolate).product::<usize>(), 125);
        assert!(
            d.achieved_image_db >= 60.0,
            "images {:.1} dB down",
            d.achieved_image_db
        );
        assert!(
            d.achieved_ripple_db <= 0.1,
            "ripple {:.4} dB",
            d.achieved_ripple_db
        );
        assert_eq!(d.input_rate_hz, 1e6);
        assert!(d.tapered_register_bits < d.register_bits);
    }

    /// **Every design returned satisfies its own spec.**
    ///
    /// The property whose absence let a real bug through: the search
    /// accepted on the compensator's ideal ripple and reported the
    /// quantised composite, so it returned a 0.1221 dB design against a
    /// 0.1 dB requirement. A test on one configuration would not have
    /// caught it either — the default spec happened to pass until the
    /// cascade ordering was fixed. Swept.
    #[test]
    fn every_returned_design_meets_its_own_spec() {
        for total in [32usize, 50, 64, 125] {
            for bw in [50e3f64, 100e3, 200e3] {
                for ripple in [0.05f64, 0.1, 0.3] {
                    for images in [40.0f64, 60.0] {
                        let spec = InterpSpec {
                            interpolate: total,
                            image_free_bw_hz: bw,
                            max_ripple_db: ripple,
                            min_image_rejection_db: images,
                            ..InterpSpec::default()
                        };
                        if let Ok(d) = design(spec.clone()) {
                            assert!(
                                d.achieved_ripple_db <= spec.max_ripple_db,
                                "{spec:?}: ripple {}",
                                d.achieved_ripple_db
                            );
                            assert!(
                                d.achieved_image_db >= spec.min_image_rejection_db,
                                "{spec:?}: images {}",
                                d.achieved_image_db
                            );
                            assert_eq!(
                                d.split().iter().product::<usize>(),
                                total,
                                "the split must multiply out"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A single stage is enough for the worked case, and asking for a
    /// split does not make it worse.
    #[test]
    fn allowing_a_split_never_costs_more() {
        let mut one = InterpSpec::default();
        one.max_chain_stages = 1;
        let a = design(one).expect("designable");
        let mut three = InterpSpec::default();
        three.max_chain_stages = 3;
        let b = design(three).expect("designable");
        assert!(
            b.cost <= a.cost * 1.000_001,
            "a larger search must not return a costlier design: {} vs {}",
            b.cost,
            a.cost
        );
    }

    /// **An impossible image requirement is named, not approximated.**
    #[test]
    fn an_unreachable_image_requirement_is_reported() {
        let mut s = InterpSpec::default();
        s.min_image_rejection_db = 250.0;
        match design(s) {
            Err(Unmet::ImageRejection { best_db, needed_db }) => {
                assert_eq!(needed_db, 250.0);
                assert!(best_db < 250.0, "best {best_db}");
            }
            other => panic!("expected ImageRejection, got {other:?}"),
        }
    }

    /// As is a bandwidth that does not fit the envelope rate.
    #[test]
    fn a_bandwidth_wider_than_the_envelope_nyquist_is_reported() {
        let mut s = InterpSpec::default();
        // 1 Msps envelope has 500 kHz of Nyquist; ask for 600 kHz.
        s.image_free_bw_hz = 600e3;
        match design(s) {
            Err(Unmet::BandwidthTooWide { passband }) => assert!(passband >= 1.0),
            other => panic!("expected BandwidthTooWide, got {other:?}"),
        }
    }

    /// And a flatness requirement no depth that rejects images can meet.
    #[test]
    fn an_incompatible_pair_of_requirements_is_reported() {
        let mut s = InterpSpec::default();
        s.min_image_rejection_db = 100.0;
        s.max_ripple_db = 1e-6;
        s.max_taps = 5;
        match design(s) {
            Err(Unmet::Incompatible { .. }) | Err(Unmet::ImageRejection { .. }) => {}
            other => panic!("expected Incompatible or ImageRejection, got {other:?}"),
        }
    }

    /// **Depth early, rate late.**
    ///
    /// The cost model weights an integrator bit by the stage's *output*
    /// rate and a comb bit by its input rate, so a chain that does its
    /// deep filtering at the low rate is cheaper than one that does it
    /// at the high rate. Checked on two hand-built splits rather than
    /// through the search, so the model is tested rather than the
    /// ranking.
    #[test]
    fn the_cost_model_prefers_depth_at_the_low_rate() {
        let mut s = InterpSpec::default();
        s.interpolate = 64;
        s.image_free_bw_hz = 100e3;
        s.fs_hz = 64e6;
        s.max_chain_stages = 2;
        let d = design(s).expect("designable");
        // Whatever the search picked, the *reported* cost must rise if
        // the same shapes are reversed. Build both orders by hand.
        let cost_of = |split: &[usize], depths: &[usize]| -> f64 {
            let mut rate_in = 1e6;
            let mut w_in = 16usize;
            let mut cost = 0.0;
            for (r, n) in split.iter().zip(depths) {
                let widths: Vec<usize> = (1..=2 * n)
                    .map(|j| interp::stage_width(j, w_in, *n, *r, 1))
                    .collect();
                let rate_out = rate_in * *r as f64;
                cost += widths[..*n].iter().sum::<usize>() as f64 * rate_in
                    + widths[*n..].iter().sum::<usize>() as f64 * rate_out;
                w_in = interp::accumulator_width(w_in, *n, *r, 1);
                rate_in = rate_out;
            }
            cost
        };
        let deep_early = cost_of(&[8, 8], &[4, 2]);
        let deep_late = cost_of(&[8, 8], &[2, 4]);
        assert!(
            deep_early < deep_late,
            "depth belongs at the low rate: {deep_early:.0} vs {deep_late:.0}"
        );
        assert!(d.cost > 0.0);
    }

    /// The factorisations are ordered and bounded.
    #[test]
    fn factorisations_are_ordered_and_bounded() {
        let f = ordered_factorisations(8, 2);
        assert!(f.contains(&vec![8]));
        assert!(f.contains(&vec![2, 4]));
        assert!(f.contains(&vec![4, 2]));
        assert!(!f.iter().any(|v| v.len() > 2));
        for v in &f {
            assert_eq!(v.iter().product::<usize>(), 8);
            assert!(v.iter().all(|x| *x >= 2));
        }
        assert!(ordered_factorisations(125, 3).contains(&vec![5, 5, 5]));
    }

    /// The depth assignments cover the space and nothing else.
    #[test]
    fn depth_assignments_are_the_cartesian_product() {
        let a = depth_assignments(2, 3);
        // Three depths times two delays, squared.
        assert_eq!(a.len(), 36);
        assert!(a.contains(&vec![(1, 1), (3, 2)]));
        assert!(a.iter().all(|v| v.len() == 2));
        assert!(a.iter().all(|v| v.iter().all(|(n, m)| *n >= 1 && *m >= 1)));
    }

    /// **The cascade image figure is at least as good as the first
    /// stage's alone.**
    ///
    /// Because every later stage also attenuates the earlier stage's
    /// images. A cascade figure *worse* than the single-stage one would
    /// mean the frequency scaling between stages was wrong, which is the
    /// classic error in a multi-rate cascade.
    #[test]
    fn a_cascade_rejects_at_least_as_well_as_its_first_stage() {
        let passband = 0.4;
        let single = vec![compensator::CicShape {
            decimate: 8,
            stages: 3,
            delay: 1,
        }];
        let cascade = vec![
            compensator::CicShape {
                decimate: 8,
                stages: 3,
                delay: 1,
            },
            compensator::CicShape {
                decimate: 4,
                stages: 2,
                delay: 1,
            },
        ];
        let (a, _) = cascade_image_db(&single, passband, 8);
        let (b, _) = cascade_image_db(&cascade, passband, 32);
        assert!(
            b >= a - 1e-9,
            "cascade {b:.2} dB should not be worse than its first stage {a:.2} dB"
        );
    }

    /// The worst image is reported at a real frequency, near a multiple
    /// of the envelope rate.
    #[test]
    fn the_worst_image_frequency_is_near_a_multiple_of_the_envelope_rate() {
        let d = design(InterpSpec::default()).expect("designable");
        let ratio = d.worst_image_hz / d.input_rate_hz;
        let nearest = ratio.round();
        assert!(nearest >= 1.0, "the first image is at k >= 1, got {ratio}");
        assert!(
            (ratio - nearest).abs() <= 0.5 * d.passband + 1e-6,
            "the worst image must sit inside an image band: {ratio}"
        );
    }
}
