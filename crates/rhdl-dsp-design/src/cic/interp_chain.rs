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
//! # Sizing a design when `R` is only known at run time
//!
//! The widget takes `R_MAX` and a runtime rate, so a design has to hold
//! for a *range*. Four things have to be checked and they behave
//! differently.
//!
//! **Widths: monotone, so `R_MAX` covers everything.**
//! [`super::interp`]'s `G_j` grows with `R`, so sizing every stage for
//! `R_MAX` is exact there and merely generous below.
//! `sizing_for_the_maximum_rate_covers_every_smaller_one` proves it.
//!
//! **Image rejection: depends on what the caller holds fixed**, and this
//! is the part that catches people. See
//! [`worst_image_over_range`] for the two regimes and their tables. With
//! a bandwidth in *hertz* — which is what
//! [`InterpSpec::image_free_bw_hz`] states — the passband fraction is
//! `2·B·R/fs` and every figure improves as `R` falls, so **`R_MAX` is
//! the worst case**. With a fixed *fractional* occupancy the worst case
//! is `R_MIN` instead, and a design verified only at the top would miss
//! a 7 dB loss by `R = 2`.
//!
//! **Ripple: the widest band's compensator serves every narrower one.**
//! The droop curve is nearly `R`-independent in `u`
//! ([`super::interp`] measures 0.027 dB between `R = 8` and `R = 125`),
//! so a smaller rate is the *same* curve over a *shorter* interval — and
//! the ripple over a sub-interval cannot exceed the ripple over the
//! whole. One tap set designed at `R_MAX` is therefore valid throughout,
//! with no per-rate switching.
//!
//! **The reachable rate set: restricted by splitting, and this is
//! arithmetic.** A stage's runtime factor tops out at its design-time
//! factor, so a `5 × 25` chain reaches only `r1 · r2` with `r1 ≤ 5` and
//! `r2 ≤ 25` — 51 of the 124 rates below 125 are unreachable, every
//! prime above 25 among them. Setting a stage to `R = 1` is part of that
//! count and does not rescue them: for a *prime* total the cap is
//! `max(per-stage factor)`, not the product.
//!
//! Worse, a rate reachable two ways gives *two different filters*,
//! because the stages' nulls sit at their own factors — and the
//! difference is large. `R = 25` on a `5 × 25` chain is `(5, 5)` at
//! 83.1 dB of image rejection or `(1, 25)` at **55.6 dB**, because
//! bypassing the first stage throws away its `sinc^5` and leaves only
//! the second stage's `sinc^2`. [`InterpDesign::verify_setting`]
//! evaluates any setting; [`InterpDesign::rates_meeting_spec`] reports
//! which rates have at least one good one — 71 of 124 against 73
//! reachable at the default design.
//!
//! So **if the rate genuinely varies, use a single stage**, which
//! [`InterpSpec::arbitrary_rate`] enforces.
//!
//! ## What a single stage costs, measured
//!
//! Not what intuition suggests. At the default configuration:
//!
//! | | single stage | `5 × 25` split |
//! |---|---|---|
//! | shape | `R=125, N=5` | `N=[5,2], M=[2,1]` |
//! | built register bits | **351** | 614 |
//! | rate-weighted cost | 2.0e10 | **9.7e9** |
//! | unreachable rates | **0** | 51 of 124 |
//!
//! The split wins on the figure that matters — half the rate-weighted
//! cost, because it does the deep filtering at 1 MHz rather than
//! 125 MHz — and *loses* on register bits, because a split chains widths
//! and its second stage takes a 31-bit input. So arbitrary rate buys
//! every rate at roughly twice the rate-weighted cost and *fewer*
//! registers. `arbitrary_rate_forces_a_single_stage` pins all of it.
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
    /// **Largest** interpolation factor the design must support.
    ///
    /// The widget's `R_MAX`: widths and counters are sized for this, and
    /// [`crate::cic::interpolator::In::rate`] chooses the rate at run
    /// time up to it. The envelope arrives at `fs_hz / R`.
    pub interpolate: usize,
    /// **Smallest** interpolation factor the design must support.
    ///
    /// Two is the default and usually right. It exists because which end
    /// of the range is the worst case depends on what the caller holds
    /// fixed — see the module docs on the two regimes — and a design
    /// cannot be verified against a range it was not told about.
    pub rate_min: usize,
    /// Must every integer rate in `rate_min..=interpolate` be reachable?
    ///
    /// **This forbids splitting the chain**, and that is not a
    /// conservatism — it is arithmetic. A two-stage chain of `5 × 25`
    /// produces only rates `r1 · r2` with `r1 ≤ 5, r2 ≤ 25`, so `R = 113`
    /// is unreachable however the stages are set. Set this when the rate
    /// is genuinely arbitrary at run time; leave it false when the caller
    /// will only ever ask for rates the split can make, and
    /// [`InterpDesign::reachable_rates`] will tell you which those are.
    pub arbitrary_rate: bool,
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
            rate_min: 2,
            arbitrary_rate: false,
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
    ///
    /// The *exact* bounds, which is the mathematical figure. See
    /// [`InterpStage::built_state_bits`] for what a generated widget
    /// spends.
    pub tapered_state_bits: usize,
    /// Register bits a `cic_interp_tapered!` widget actually carries.
    ///
    /// The exact bounds are not monotonic, and a generated datapath uses
    /// the running maximum so that every inter-stage transfer is a
    /// widening — one bit more in practice, and no truncation logic
    /// anywhere. This is the number to build to;
    /// [`InterpStage::tapered_state_bits`] is the number to quote as the
    /// bound.
    pub built_state_bits: usize,
    /// Per-stage widths as built: the monotone lift of
    /// [`InterpStage::stage_widths`].
    pub built_widths: Vec<usize>,
}

/// A cheaper or otherwise notable candidate that was not chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Alternative {
    /// How the interpolation was split.
    pub split: Vec<usize>,
    /// Per-stage CIC depth.
    pub stages: Vec<usize>,
    /// Per-stage differential delay.
    ///
    /// Recorded because without it the runner-up can print identically
    /// to the winner: two candidates differing only in `M` are genuinely
    /// different filters with different costs, and the example's output
    /// read as "split [5, 25] N=[5, 2]" twice until this field existed.
    pub delays: Vec<usize>,
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
    /// Achieved worst image over the whole rate range, in dB below the
    /// signal. Positive.
    ///
    /// The **worst** figure across `rate_min..=interpolate`, not the
    /// figure at one rate — a design that met the spec at `R_MAX` and
    /// missed it at `R_MIN` would otherwise be reported as passing.
    pub achieved_image_db: f64,
    /// The rate at which [`InterpDesign::achieved_image_db`] was worst.
    ///
    /// Which end this lands on is the whole story of the two regimes;
    /// see the module docs.
    pub worst_image_rate: usize,
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
    /// Total register bits under the lossless taper, at the exact
    /// bounds.
    pub tapered_register_bits: usize,
    /// Total register bits a generated tapered widget carries.
    pub built_register_bits: usize,
    /// Width the pre-compensator's output needs to be unable to saturate
    /// on **any** input.
    ///
    /// `input_width + ceil(log2 ‖h‖₁)`, from the sum of the quantised
    /// taps' magnitudes. The `l1` norm is the bound for an arbitrary
    /// bounded input, so a register this wide cannot overflow whatever
    /// arrives.
    ///
    /// **This is the one to build to unless you know the input is
    /// band-limited.** A transmit envelope is not: switch-on, a burst
    /// boundary and a modulation change are all steps, and a step is
    /// exactly the input that reaches the `l1` bound.
    pub mid_width_any_input: usize,
    /// Width it needs to be unable to saturate on a signal confined to
    /// the passband.
    ///
    /// `input_width + ceil(log2 peak)`, from the largest passband gain.
    /// Narrower than [`InterpDesign::mid_width_any_input`] — often by
    /// two or three bits — and correct only for an in-band sinusoid.
    ///
    /// Reported because it is the figure the follow-up that prompted
    /// this asked for, and because for a continuously-modulated carrier
    /// it is the right one. Choosing it for a burst transmitter is how a
    /// compensator saturates on the first sample.
    pub mid_width_in_band: usize,
    /// Sum of the quantised taps' magnitudes, `‖h‖₁`.
    pub compensator_l1: f64,
    /// Largest passband gain the compensator asks for.
    pub compensator_peak: f64,
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

    /// Per-stage differential delay, lowest rate first.
    pub fn delays(&self) -> Vec<usize> {
        self.cics.iter().map(|c| c.delay).collect()
    }

    /// Every runtime rate this chain can actually produce.
    ///
    /// **A split restricts the rate set, and that is arithmetic rather
    /// than conservatism.** Each stage's runtime rate is independently
    /// settable from one up to its own factor, so the reachable totals
    /// are the products — a `5 × 25` chain reaches 1 through 125 but
    /// *only* at values expressible as `r1 · r2` with `r1 ≤ 5` and
    /// `r2 ≤ 25`. `R = 113` is prime and larger than 5, so no setting
    /// produces it.
    ///
    /// A single-stage design reaches every integer up to its factor,
    /// which is why [`InterpSpec::arbitrary_rate`] forces one.
    ///
    /// Sorted and deduplicated.
    pub fn reachable_rates(&self) -> Vec<usize> {
        let mut set: Vec<usize> = vec![1];
        for c in &self.cics {
            let mut next = Vec::new();
            for base in &set {
                for r in 1..=c.interpolate {
                    next.push(base * r);
                }
            }
            next.sort_unstable();
            next.dedup();
            set = next;
        }
        set.retain(|r| *r >= 1);
        set
    }

    /// What a given per-stage rate setting actually achieves.
    ///
    /// # Why this is not the same as the design's headline figures
    ///
    /// The design's `achieved_image_db` and `achieved_ripple_db` describe
    /// the configuration it was designed *for* — every stage at its
    /// design-time factor. A chain with a run-time rate can be set to
    /// others, and at those settings the *shapes change*, not just the
    /// band: a stage's nulls sit at multiples of whatever factor it is
    /// set to.
    ///
    /// # The `R = 1` trick, and where it stops being free
    ///
    /// Setting a stage to one is the obvious way to reach more rates, and
    /// [`InterpDesign::reachable_rates`] already counts those settings.
    /// It is genuinely free **only when that stage has `M = 1`**, where
    /// `(1 - z^-1)^N` and `1/(1 - z^-1)^N` cancel exactly and the stage
    /// is a delay.
    ///
    /// At `M = 2` they do not cancel. The stage becomes
    /// `(1 + z^-1)^N`, whose magnitude is `|cos(π f)|^N` — an `N`-th
    /// order lowpass with all its zeros at Nyquist — **and its gain is
    /// `M^N`, not one**. At `N = 5, M = 2` that is 0.78 at a tenth of
    /// Nyquist, 0.18 at a quarter, and a gain of 32. The compensator was
    /// not designed against that curve, so the passband is neither flat
    /// nor correctly scaled.
    ///
    /// This matters here because [`design`]'s search is free to choose
    /// `M = 2`, and at the default configuration it does. So "just set a
    /// stage to one" is sound advice for an `M = 1` stage and a trap for
    /// an `M = 2` one — which is what this function is for.
    /// `a_stage_at_rate_one_is_only_free_at_unit_delay` measures it.
    pub fn verify_setting(&self, per_stage: &[usize]) -> Option<SettingReport> {
        if per_stage.len() != self.cics.len() {
            return None;
        }
        if per_stage
            .iter()
            .zip(&self.cics)
            .any(|(r, c)| *r == 0 || *r > c.interpolate)
        {
            return None;
        }
        let total: usize = per_stage.iter().product();
        if total == 0 {
            return None;
        }
        let shapes: Vec<compensator::CicShape> = per_stage
            .iter()
            .zip(&self.cics)
            .map(|(r, c)| compensator::CicShape {
                decimate: *r,
                stages: c.stages,
                delay: c.delay,
            })
            .collect();

        let passband = 2.0 * self.spec.image_free_bw_hz * total as f64 / self.spec.fs_hz;
        if !(passband > 0.0 && passband < 1.0) {
            return None;
        }
        let (image_db, _) = cascade_image_db(&shapes, passband, total);
        let ripple_db = combined_ripple_db(&shapes, passband, &dequantised(&self.compensator));
        let gain: f64 = per_stage
            .iter()
            .zip(&self.cics)
            .map(|(r, c)| {
                let (num, den) = interp::dc_gain_ratio(c.stages, *r, c.delay);
                num as f64 / den as f64
            })
            .product();

        Some(SettingReport {
            per_stage: per_stage.to_vec(),
            total,
            input_rate_hz: self.spec.fs_hz / total as f64,
            image_db,
            ripple_db,
            gain,
            meets_spec: image_db >= self.spec.min_image_rejection_db
                && ripple_db <= self.spec.max_ripple_db,
        })
    }

    /// Every per-stage setting whose product is `rate`.
    ///
    /// More than one for a composite rate: `R = 20` on a `5 × 25` chain
    /// is `(1, 20)`, `(2, 10)`, `(4, 5)` and `(5, 4)`, and they are four
    /// *different filters*. [`InterpDesign::verify_setting`] says which
    /// of them still meet the spec.
    pub fn settings_for(&self, rate: usize) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        fn go(caps: &[usize], left: usize, acc: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            if caps.is_empty() {
                if left == 1 {
                    out.push(acc.clone());
                }
                return;
            }
            for r in 1..=caps[0] {
                if left.is_multiple_of(r) {
                    acc.push(r);
                    go(&caps[1..], left / r, acc, out);
                    acc.pop();
                }
            }
        }
        let caps: Vec<usize> = self.cics.iter().map(|c| c.interpolate).collect();
        go(&caps, rate, &mut Vec::new(), &mut out);
        out
    }

    /// Rates that are reachable **and** have at least one setting meeting
    /// the spec.
    ///
    /// The honest answer to "which rates can I actually use".
    /// [`InterpDesign::reachable_rates`] counts what the counters can
    /// produce; this counts what the *filter* still delivers, which is
    /// smaller whenever a stage has `M > 1` — see
    /// [`InterpDesign::verify_setting`].
    pub fn rates_meeting_spec(&self) -> Vec<usize> {
        self.reachable_rates()
            .into_iter()
            .filter(|r| *r >= self.spec.rate_min && *r <= self.spec.interpolate)
            .filter(|r| {
                self.settings_for(*r)
                    .iter()
                    .filter_map(|s| self.verify_setting(s))
                    .any(|rep| rep.meets_spec)
            })
            .collect()
    }

    /// Rates in `rate_min..=interpolate` this chain **cannot** produce.
    ///
    /// Empty for a single-stage design. For a split, the list a caller
    /// has to live with — or set [`InterpSpec::arbitrary_rate`] and take
    /// the wider registers instead.
    pub fn unreachable_rates(&self) -> Vec<usize> {
        let ok = self.reachable_rates();
        (self.spec.rate_min.max(1)..=self.spec.interpolate)
            .filter(|r| !ok.contains(r))
            .collect()
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

/// What a particular per-stage rate setting actually achieves.
///
/// Returned by [`InterpDesign::verify_setting`]. The point of it is that
/// a design's headline figures describe the configuration it was
/// designed *for*, and a chain with a run-time rate can be set to
/// others.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingReport {
    /// Per-stage rates, lowest-rate stage first.
    pub per_stage: Vec<usize>,
    /// Total interpolation, the product.
    pub total: usize,
    /// Envelope rate this setting implies, in Hz.
    pub input_rate_hz: f64,
    /// Worst image at this setting, in dB below the signal.
    pub image_db: f64,
    /// Composite passband ripple at this setting, in dB.
    pub ripple_db: f64,
    /// Signal gain at this setting, as a float.
    ///
    /// Not the design's gain: a stage set to a different factor has a
    /// different `(R·M)^N / R`, and a stage with `M > 1` set to `R = 1`
    /// has a gain of `M^N` rather than one.
    pub gain: f64,
    /// Does this setting still meet the spec it was designed against?
    pub meets_spec: bool,
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

/// Worst image rejection over a whole rate range, and where it occurred.
///
/// # Which end is the worst case depends on what the caller holds fixed
///
/// This is the question a run-time-variable rate forces, and it has two
/// answers.
///
/// **Fixed absolute bandwidth** — the caller has a 200 kHz signal and a
/// 125 MHz converter, and varies `R` to trade host bandwidth. Then the
/// passband *fraction* is `2·B·R/fs`, proportional to `R`, so a smaller
/// rate puts the signal a smaller fraction of the way to the first null.
/// Everything improves monotonically as `R` falls:
///
/// | `R` | passband | images | droop |
/// |---|---|---|---|
/// | 125 | 0.400 | 37.9 dB | −1.74 dB |
/// | 64 | 0.205 | 57.0 dB | −0.45 dB |
/// | 16 | 0.051 | 94.7 dB | −0.03 dB |
/// | 2 | 0.006 | 137.9 dB | −0.00 dB |
///
/// So **`R_MAX` is the worst case and designing there guarantees the
/// rest**, which is what [`design`] does.
///
/// **Fixed fractional occupancy** — the caller always fills 40% of the
/// envelope Nyquist, so the absolute bandwidth scales as `1/R`. Then the
/// passband fraction is constant and the figures are nearly
/// `R`-independent — until they are not:
///
/// | `R` | images |
/// |---|---|
/// | 125 | 37.9 dB |
/// | 16 | 37.8 dB |
/// | 8 | 37.4 dB |
/// | 4 | 36.1 dB |
/// | 2 | **30.6 dB** |
///
/// Flat to within 0.5 dB down to `R = 8`, then 7 dB of loss by `R = 2`,
/// because `sin(π u / R)` stops being its argument. **Here the worst
/// case is `R_MIN`.**
///
/// # Single-stage designs only
///
/// At run time a stage's factor *is* the rate it is set to, so its nulls
/// move — the response is a different curve, not the same curve over a
/// narrower band. For one stage that is easy: rebuild the shape with the
/// runtime factor, which is what this does.
///
/// For a **split** it is not, because the runtime rate can be factorised
/// across the stages in more than one way and the response differs for
/// each. `5 × 25` set to `(5, 4)` and to `(1, 20)` both give `R = 20` and
/// are different filters. So a split is left at its design-time shape
/// here, and the honest position is the one
/// [`InterpSpec::arbitrary_rate`] encodes: **if the rate varies at run
/// time, use a single stage.** A split fixes the rate at design time, or
/// restricts it to a set the caller verifies themselves —
/// [`InterpDesign::reachable_rates`] says which set.
pub fn worst_image_over_range(
    shapes: &[compensator::CicShape],
    fs_hz: f64,
    bw_hz: f64,
    rate_min: usize,
    rate_max: usize,
) -> (f64, usize) {
    let mut worst = f64::INFINITY;
    let mut at = rate_max;
    for r in rate_min.max(1)..=rate_max {
        let pb = 2.0 * bw_hz * r as f64 / fs_hz;
        if !(pb > 0.0 && pb < 1.0) {
            continue;
        }
        // The cascade's shapes are fixed; only the band moves.
        let (db, _) = cascade_image_db(shapes, pb, r);
        if db < worst {
            worst = db;
            at = r;
        }
    }
    (worst, at)
}

/// Where a **post**-compensator's passband and stopband sit, in
/// converter-rate units.
///
/// Returns `(passband_edge, stopband_edge)` as fractions of the
/// converter rate. A FIR placed *after* the interpolator runs at `fs`,
/// so the signal it must pass has been squeezed into `[0, edge/R]` and
/// the first image it must stop begins at `(1 - edge)/R`.
///
/// That squeezing is the whole story of post-compensation: the filter
/// gets narrower in proportion to `R`, and a narrow filter costs taps.
pub fn post_compensator_bands(passband: f64, r: usize) -> (f64, f64) {
    let edge = response::passband_edge_out(passband);
    let rr = r as f64;
    (edge / rr, (1.0 - edge) / rr)
}

/// Roughly how many taps a post-compensator needs, by the Kaiser
/// estimate.
///
/// `N ≈ (A - 8) / (2.285 · Δω)`, with `Δω` the transition band in
/// radians per converter sample. **An estimate and not a design** — it
/// ignores the droop the filter also has to invert, which pushes the
/// real number up — but it is the right order and it is enough to answer
/// the question a caller actually has, which is whether a
/// post-compensator is affordable at all.
///
/// # It is usually not, and this is the function that says so
///
/// The transition band is `(1 - 2·edge)/R` wide, so the tap count grows
/// **linearly with `R`** — and every one of those taps runs at the
/// converter clock. Measured at `passband = 0.4`, 60 dB:
///
/// | `R` | taps |
/// |---|---|
/// | 2 | 12 |
/// | 4 | 24 |
/// | 8 | 48 |
/// | 16 | 97 |
/// | 32 | 195 |
/// | 125 | 755 |
///
/// So a post-compensator is the right shape at `R` up to about eight,
/// and at `R = 125` it is a 755-tap FIR at 125 MHz, which is not a
/// widget anybody wants. `the_tap_count_grows_with_the_rate` pins the
/// table.
///
/// **The useful reading is not "don't", it is "not here".** Put the
/// post-compensator between *chain stages*, where the local `R` is
/// small, rather than after the whole interpolation. A `5 × 25` split
/// compensated after its first stage runs a filter at 5 MHz with an `R`
/// of 5, not at 125 MHz with an `R` of 125. That is what
/// [`InterpSpec::max_chain_stages`] is for, and it is the reason
/// splitting a transmit chain buys more than register bits.
pub fn post_compensator_taps(passband: f64, r: usize, attenuation_db: f64) -> usize {
    let (f_pass, f_stop) = post_compensator_bands(passband, r);
    let dw = std::f64::consts::TAU * (f_stop - f_pass);
    if dw <= 0.0 {
        return usize::MAX;
    }
    let n = (attenuation_db - 8.0) / (2.285 * dw);
    let n = n.ceil().max(3.0) as usize;
    // Symmetric FIRs need a centre tap.
    if n.is_multiple_of(2) { n + 1 } else { n }
}

/// Design a **post**-compensator: a converter-rate FIR that both
/// flattens the band and attenuates the images.
///
/// The taps a
/// [`crate::cic::post_compensated_interp`](https://docs.rs/rhdl-fpga)
/// widget needs. Returns `None` if the fit is not designable at this tap
/// count — an even count, or a band reaching a null.
///
/// # Why this is not [`compensator::design`]
///
/// A post-compensator runs at the converter rate while the droop it
/// inverts is a property of the envelope rate, so its own frequency
/// variable and the cascade's differ by `R`.
/// [`compensator::design_scaled`] is the general form that takes that
/// factor; this function computes the band edges in converter-rate units
/// and hands it `R`.
///
/// # And it is the only compensator that can touch the images
///
/// A *pre*-compensator is periodic with period one in envelope-rate
/// units, so it lifts every image exactly as much as the signal. Moving
/// the filter to the converter rate breaks the periodicity, so `u` and
/// `k + u` become different frequencies to it and `min_stopband_db`
/// becomes a real image requirement rather than a no-op.
///
/// **Check [`post_compensator_taps`] before reaching for this.** The
/// filter narrows in proportion to `R`, so the cost is linear in the
/// rate and at `R = 125` it is a 755-tap FIR at the converter clock.
/// Between chain stages, where the local `R` is small, it is a couple of
/// dozen taps.
pub fn post_compensator(
    shapes: &[compensator::CicShape],
    passband: f64,
    r: usize,
    taps: usize,
    min_stopband_db: f64,
    coeff_width: usize,
) -> Option<compensator::Quantised> {
    let (f_pass, f_stop) = post_compensator_bands(passband, r);
    if f_stop >= 0.5 || f_pass <= 0.0 {
        return None;
    }
    let spec = compensator::Spec {
        cics: evaluation_order(shapes),
        // `passband_edge_out` halves, so double going in: these are
        // fractions of the *converter* Nyquist now.
        passband: 2.0 * f_pass,
        taps,
        stopband_edge: 2.0 * f_stop,
        min_stopband_db,
        max_ripple_db: 1.0,
        // Least squares only: the exchange algorithm's band bookkeeping
        // assumes the filter and the cascade share a frequency variable.
        // See `compensator::design_scaled`.
        method: compensator::Method::LeastSquares,
    };
    let d = compensator::design_scaled(spec, r as f64)?;
    Some(compensator::quantise(&d, coeff_width))
}

/// Composite image rejection with a post-compensator in place, in dB
/// below the signal.
///
/// [`cascade_image_db`] answers the same question for the cascade alone.
/// This one multiplies in the converter-rate filter, which — unlike a
/// pre-compensator — actually changes the answer.
///
/// `taps` are in converter-rate units, as
/// [`post_compensator`] produces them.
pub fn post_compensated_image_db(
    shapes: &[compensator::CicShape],
    passband: f64,
    r: usize,
    taps: &[f64],
) -> f64 {
    let order = evaluation_order(shapes);
    let edge = response::passband_edge_out(passband);
    let rr = r as f64;
    // Reference: the composite at DC, which is where the passband is
    // normalised.
    let at = |g: f64| {
        compensator::cascade_magnitude(&order, g * rr) * compensator::fir_amplitude(taps, g).abs()
    };
    let reference = at(0.0);
    let mut worst = 0.0f64;
    for k in 1..=(r / 2).max(1) {
        const STEPS: usize = 257;
        for s in 0..STEPS {
            let u = k as f64 - edge + 2.0 * edge * (s as f64 / (STEPS - 1) as f64);
            let g = u / rr;
            if g <= 0.0 || g > 0.5 {
                continue;
            }
            worst = worst.max(at(g));
        }
    }
    if worst <= 1e-15 || reference <= 1e-15 {
        300.0
    } else {
        -20.0 * (worst / reference).log10()
    }
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
pub fn evaluation_shapes_of(shapes: &[compensator::CicShape]) -> Vec<compensator::CicShape> {
    evaluation_order(shapes)
}

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

/// Headroom the compensator's output needs, both bounds.
///
/// Returns `(any_input, in_band, l1, peak)`: the two widths and the two
/// norms they come from. Measured on the taps the hardware holds, not on
/// the ideal design — quantisation moves both slightly and the register
/// has to hold what is actually computed.
pub fn compensator_headroom_of(
    input_width: usize,
    taps: &[f64],
    passband: f64,
) -> (usize, usize, f64, f64) {
    let l1: f64 = taps.iter().map(|t| t.abs()).sum();
    let edge = response::passband_edge_out(passband);
    let mut peak = 0.0f64;
    const GRID: usize = 512;
    for g in 0..GRID {
        let u = edge * g as f64 / (GRID - 1) as f64;
        peak = peak.max(compensator::fir_amplitude(taps, u).abs());
    }
    let bits = |gain: f64| -> usize {
        if gain <= 1.0 {
            input_width
        } else {
            input_width + gain.log2().ceil() as usize
        }
    };
    (bits(l1), bits(peak), l1, peak)
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

    if spec.rate_min < 1 || spec.rate_min > spec.interpolate {
        return Err(Unmet::Invalid {
            reason: "rate_min must be between one and the maximum rate",
        });
    }

    let input_rate_hz = spec.fs_hz / spec.interpolate as f64;
    // `arbitrary_rate` forbids splitting: only a single stage reaches
    // every integer rate. See `InterpDesign::reachable_rates`.
    let chain_stages = if spec.arbitrary_rate {
        1
    } else {
        spec.max_chain_stages.max(1)
    };
    let splits = ordered_factorisations(spec.interpolate, chain_stages);

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

            // Evaluated at `R_MAX` alone, and that is the worst case
            // rather than a shortcut.
            //
            // With the bandwidth stated in hertz the passband *fraction*
            // is `2·B·R/fs`, proportional to the rate, so a smaller rate
            // puts the signal a smaller fraction of the way to the first
            // null and every figure improves: 37.9 dB of image rejection
            // at `R = 125` becomes 137.9 dB at `R = 2`.
            // `worst_image_over_range` carries the table and
            // `image_rejection_is_monotonic_in_the_rate` sweeps it
            // densely.
            //
            // Sweeping *here* instead was tried and reverted. It is a
            // search inside a search: the test module went from 19
            // seconds to 514. The guarantee belongs in a test that runs
            // once, not in every candidate evaluation.
            let (image_db, at_u) = cascade_image_db(&shapes, passband, spec.interpolate);
            let worst_rate = spec.interpolate;
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
            let mut built_bits = 0usize;
            for (r, (n, m)) in split.iter().zip(&depths) {
                let wa = interp::accumulator_width(w_in, *n, *r, *m);
                let widths: Vec<usize> = (1..=2 * n)
                    .map(|j| interp::stage_width(j, w_in, *n, *r, *m))
                    .collect();
                let u_bits = interp::uniform_state_bits(w_in, *n, *r, *m);
                let t_bits = interp::tapered_state_bits(w_in, *n, *r, *m);
                let b_bits = interp::implemented_state_bits(w_in, *n, *r, *m);
                let built: Vec<usize> = (1..=2 * n)
                    .map(|j| interp::implemented_stage_width(j, w_in, *n, *r, *m))
                    .collect();
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
                built_bits += b_bits;
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
                    built_state_bits: b_bits,
                    built_widths: built,
                });
                w_in = wa;
                rate_in = rate_out;
            }

            let (mid_any, mid_band, l1, peak) =
                compensator_headroom_of(spec.input_width, &dequantised(&quantised), passband);

            feasible.push(InterpDesign {
                spec: spec.clone(),
                cics: stages_out,
                compensator: quantised.clone(),
                passband,
                input_rate_hz,
                achieved_ripple_db: achieved_ripple,
                achieved_image_db: image_db,
                worst_image_rate: worst_rate,
                worst_image_hz: at_u * spec.fs_hz / worst_rate as f64,
                // A full-scale sine at the converter width. The chain
                // adds nothing: the interpolator is exact.
                dac_snr_db: 6.02 * spec.output_width as f64 + 1.76,
                cost,
                register_bits: uniform_bits,
                tapered_register_bits: tapered_bits,
                built_register_bits: built_bits,
                mid_width_any_input: mid_any,
                mid_width_in_band: mid_band,
                compensator_l1: l1,
                compensator_peak: peak,
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
        delays: d.delays(),
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

    /// **Image rejection improves monotonically as the rate falls**, for
    /// a bandwidth stated in hertz.
    ///
    /// The guarantee [`design`] leans on to evaluate at `R_MAX` alone.
    /// Swept densely here, once, rather than inside every candidate
    /// evaluation — which is where it was first put, and it took this
    /// module from 19 seconds to 514.
    ///
    /// The mechanism is that the passband *fraction* is `2·B·R/fs`, so a
    /// smaller rate puts the signal a smaller fraction of the way to the
    /// first null. Note the shape is rebuilt at each rate: a stage's
    /// factor *is* the rate it is set to, so its nulls move too.
    #[test]
    fn image_rejection_is_monotonic_in_the_rate() {
        let (fs, bw, m) = (125e6f64, 200e3f64, 1usize);
        for n in 1..=5 {
            let mut prev: Option<(usize, f64)> = None;
            for r in 2..=125usize {
                let pb = 2.0 * bw * r as f64 / fs;
                if !(pb > 0.0 && pb < 1.0) {
                    continue;
                }
                let db = worst_image_db(pb, n, r, m);
                if let Some((pr, pd)) = prev {
                    assert!(
                        db <= pd + 1e-6,
                        "N={n}: R={r} gives {db:.2} dB, worse than R={pr}'s {pd:.2}"
                    );
                }
                prev = Some((r, db));
            }
        }
        // And the span is large, so the monotonicity is doing real work
        // rather than being a flat line.
        let at = |r: usize| worst_image_db(2.0 * bw * r as f64 / fs, 3, r, m);
        assert!(at(2) > at(125) + 80.0, "{} vs {}", at(2), at(125));
    }

    /// **`worst_image_over_range` agrees that the maximum is the worst.**
    ///
    /// The function `design` does *not* call, kept as a verification
    /// helper a caller can run. If this ever disagreed with the fast
    /// path, one of them would be wrong.
    #[test]
    fn the_worst_case_is_the_maximum_rate() {
        let shapes = vec![compensator::CicShape {
            decimate: 125,
            stages: 3,
            delay: 1,
        }];
        let (db, at) = worst_image_over_range(&shapes, 125e6, 200e3, 2, 125);
        assert_eq!(at, 125, "the worst rate is the maximum");
        let (direct, _) = cascade_image_db(&shapes, 2.0 * 200e3 * 125.0 / 125e6, 125);
        assert!(
            (db - direct).abs() < 1e-9,
            "the swept figure {db:.4} must match the direct one {direct:.4}"
        );
    }

    /// **The other regime degrades at the *low* end**, which is why the
    /// distinction is in the docs rather than assumed away.
    ///
    /// If the caller holds the *fractional* occupancy fixed instead —
    /// always 40% of the envelope Nyquist, so the absolute bandwidth
    /// scales as `1/R` — the figures are nearly rate-independent down to
    /// about `R = 8` and then fall away. A design verified only at
    /// `R_MAX` would miss it.
    #[test]
    fn a_fixed_fractional_band_degrades_at_small_rates() {
        let at = |r: usize| worst_image_db(0.4, 3, r, 1);
        // Flat to within half a dB down to eight.
        for r in [8usize, 16, 32, 64, 125] {
            assert!(
                (at(r) - at(125)).abs() < 0.5,
                "R={r}: {:.2} against {:.2} at 125",
                at(r),
                at(125)
            );
        }
        // Then it goes: about 7 dB by R = 2.
        assert!(
            at(125) - at(2) > 5.0,
            "R=2 gives {:.2}, R=125 gives {:.2}",
            at(2),
            at(125)
        );
    }

    /// **A split cannot reach every rate, and the design says which.**
    ///
    /// Arithmetic, not conservatism: a `5 × 25` chain produces only
    /// `r1 · r2` with `r1 ≤ 5` and `r2 ≤ 25`, so no setting gives
    /// `R = 113`.
    #[test]
    fn a_split_restricts_the_reachable_rates() {
        let spec = InterpSpec {
            max_chain_stages: 2,
            ..InterpSpec::default()
        };
        let d = design(spec).expect("designable");
        let reachable = d.reachable_rates();
        let missing = d.unreachable_rates();
        if d.cics.len() > 1 {
            assert!(
                !missing.is_empty(),
                "a split of {:?} should miss some rates",
                d.split()
            );
            // 113 is prime and larger than either factor of a 5 x 25
            // split, so it is unreachable however the stages are set.
            for r in &missing {
                assert!(!reachable.contains(r));
            }
            assert!(
                reachable.contains(&d.spec.interpolate),
                "R_MAX itself works"
            );
        }
        // Whatever the split, the total is always reachable.
        assert!(reachable.contains(&125));
    }

    /// **A stage set to `R = 1` is only a bypass at `M = 1`.**
    ///
    /// The answer to "can't I just run one stage at one to reach the
    /// missing rates". You can, and `reachable_rates` already counts
    /// those settings — but at `M = 2` the stage stops being a
    /// pass-through: `(1 - z^-2)^N / (1 - z^-1)^N` is `(1 + z^-1)^N`,
    /// whose magnitude is `|cos(π f)|^N`, and its gain is `M^N`.
    #[test]
    fn a_stage_at_rate_one_is_only_free_at_unit_delay() {
        for n in [1usize, 2, 5] {
            // M = 1: flat everywhere, gain one. A genuine bypass.
            for f in [0.0f64, 0.1, 0.25, 0.5] {
                let h = response::magnitude(f, n, 1, 1);
                assert!((h - 1.0).abs() < 1e-12, "M=1 N={n} f={f}: {h}");
            }
            assert_eq!(interp::dc_gain_ratio(n, 1, 1), (1, 1));

            // M = 2: an N-th order lowpass with every zero at Nyquist.
            assert!(
                (response::magnitude(0.0, n, 1, 2) - 1.0).abs() < 1e-12,
                "still unity at DC"
            );
            assert!(
                response::magnitude(0.5, n, 1, 2) < 1e-12,
                "and zero at Nyquist"
            );
            let quarter = response::magnitude(0.25, n, 1, 2);
            let expected = 0.5f64.sqrt().powi(n as i32);
            assert!(
                (quarter - expected).abs() < 1e-9,
                "M=2 N={n} at a quarter: {quarter}, expected |cos(pi/4)|^N = {expected}"
            );
            // And the gain is M^N, not one.
            let (num, den) = interp::dc_gain_ratio(n, 1, 2);
            assert_eq!((num, den), (1u128 << n, 1), "M=2 N={n} gain");
        }
        // The headline number: at N = 5, M = 2 a "bypassed" stage is a
        // 5th-order lowpass with a gain of 32.
        assert!((response::magnitude(0.25, 5, 1, 2) - 0.1768).abs() < 1e-3);
        assert_eq!(interp::dc_gain_ratio(5, 1, 2), (32, 1));
    }

    /// **And the trick cannot reach a prime above the largest stage.**
    ///
    /// The other half of the answer. `R = 1` on one stage leaves the
    /// total equal to the other stage's setting, so for a prime total the
    /// cap is `max(per-stage factors)`, not their product. `29` is prime
    /// and larger than 25, so no setting of a `5 × 25` chain produces it.
    #[test]
    fn the_rate_one_trick_is_capped_by_the_largest_stage() {
        let d = design(InterpSpec {
            max_chain_stages: 2,
            ..InterpSpec::default()
        })
        .expect("designable");
        if d.cics.len() < 2 {
            return;
        }
        let caps: Vec<usize> = d.cics.iter().map(|c| c.interpolate).collect();
        let biggest = *caps.iter().max().unwrap();
        let reachable = d.reachable_rates();
        // Every prime above the largest stage is out of reach, however
        // the stages are set.
        for p in [29usize, 31, 37, 41, 43, 47, 53, 59, 61] {
            if p > biggest && p <= d.spec.interpolate {
                assert!(
                    !reachable.contains(&p),
                    "prime {p} exceeds the largest stage {biggest} and cannot be reached"
                );
                assert!(d.settings_for(p).is_empty(), "no setting gives {p}");
            }
        }
        // But a composite below the product is, via the R = 1 setting.
        assert!(reachable.contains(&biggest), "the largest stage alone");
        assert!(
            !d.settings_for(biggest).is_empty(),
            "and there is a setting for it"
        );
    }

    /// **One rate, several settings, several different filters.**
    ///
    /// `R = 20` on a `5 × 25` chain is `(1,20)`, `(2,10)`, `(4,5)` and
    /// `(5,4)`. They are not interchangeable — each stage's nulls sit at
    /// its own factor — so "the rate is 20" does not determine the
    /// response.
    #[test]
    fn one_rate_can_have_several_settings_that_differ() {
        let d = design(InterpSpec {
            max_chain_stages: 2,
            ..InterpSpec::default()
        })
        .expect("designable");
        if d.cics.len() < 2 {
            return;
        }
        let settings = d.settings_for(20);
        assert!(settings.len() > 1, "several settings give 20: {settings:?}");
        let reports: Vec<SettingReport> = settings
            .iter()
            .filter_map(|s| d.verify_setting(s))
            .collect();
        assert_eq!(reports.len(), settings.len());
        for r in &reports {
            assert_eq!(r.total, 20);
        }
        // They genuinely differ -- in image rejection, in gain, or both.
        let images: Vec<f64> = reports.iter().map(|r| r.image_db).collect();
        let gains: Vec<f64> = reports.iter().map(|r| r.gain).collect();
        let differ = images.windows(2).any(|w| (w[0] - w[1]).abs() > 0.01)
            || gains.windows(2).any(|w| (w[0] - w[1]).abs() > 0.01);
        assert!(
            differ,
            "settings for one rate must not all be the same filter: \
             images {images:?} gains {gains:?}"
        );
    }

    /// **`rates_meeting_spec` is smaller than `reachable_rates`.**
    ///
    /// Which is the honest answer to "which rates can I use": the
    /// counters reach more than the filter delivers.
    #[test]
    fn the_usable_rates_are_fewer_than_the_reachable_ones() {
        let d = design(InterpSpec {
            max_chain_stages: 2,
            ..InterpSpec::default()
        })
        .expect("designable");
        let reachable: Vec<usize> = d
            .reachable_rates()
            .into_iter()
            .filter(|r| *r >= d.spec.rate_min && *r <= d.spec.interpolate)
            .collect();
        let usable = d.rates_meeting_spec();
        assert!(
            usable.len() <= reachable.len(),
            "usable {} cannot exceed reachable {}",
            usable.len(),
            reachable.len()
        );
        // The design's own rate is always usable -- it is what was
        // designed.
        assert!(
            usable.contains(&d.spec.interpolate),
            "the design rate must be usable, usable = {usable:?}"
        );
        // And a setting the design never considered is verifiable.
        let all_one: Vec<usize> = vec![1; d.cics.len()];
        let rep = d.verify_setting(&all_one).expect("R = 1 everywhere");
        assert_eq!(rep.total, 1);
    }

    /// **And `arbitrary_rate` forces a single stage, which reaches
    /// everything.**
    ///
    /// The escape hatch for a genuinely run-time rate: wider registers
    /// in exchange for every integer being settable.
    #[test]
    fn arbitrary_rate_forces_a_single_stage() {
        let spec = InterpSpec {
            arbitrary_rate: true,
            max_chain_stages: 3,
            ..InterpSpec::default()
        };
        let d = design(spec).expect("designable");
        assert_eq!(d.cics.len(), 1, "split {:?}", d.split());
        assert_eq!(d.split(), vec![125]);
        assert!(
            d.unreachable_rates().is_empty(),
            "a single stage reaches every rate, missing {:?}",
            d.unreachable_rates()
        );
        // **And what it costs is not what you would guess.**
        //
        // Measured at the default configuration: the single stage picks
        // `R = 125, N = 5` and spends 351 built register bits; the
        // `5 × 25` split picks `N = [5, 2]` and spends 614, because a
        // split *chains* widths — its second stage takes a 31-bit input,
        // so every stage of it is at least 31 bits wide.
        //
        // The split wins on the figure that actually matters, which is
        // the rate-weighted cost: 9.7e9 against 2.0e10, less than half,
        // because it does the deep filtering at 1 MHz instead of at
        // 125 MHz.
        //
        // So `arbitrary_rate` buys every rate at roughly **twice the
        // rate-weighted cost and fewer registers** — not "wider
        // registers", which is what an earlier version of this test
        // asserted and got backwards.
        let split = design(InterpSpec {
            max_chain_stages: 2,
            ..InterpSpec::default()
        })
        .expect("designable");
        if split.cics.len() > 1 {
            assert!(
                d.cost > split.cost,
                "the split should be cheaper by the rate-weighted model: \
                 {:.3e} against {:.3e}",
                d.cost,
                split.cost
            );
            assert!(
                d.built_register_bits < split.built_register_bits,
                "and the single stage should spend *fewer* registers: \
                 {} against {}",
                d.built_register_bits,
                split.built_register_bits
            );
            assert!(
                !split.unreachable_rates().is_empty(),
                "which is what the split gives up"
            );
        }
    }

    /// A `rate_min` outside the range is refused.
    #[test]
    fn an_invalid_rate_range_is_refused() {
        for rate_min in [0usize, 200] {
            let spec = InterpSpec {
                rate_min,
                ..InterpSpec::default()
            };
            assert!(
                matches!(design(spec), Err(Unmet::Invalid { .. })),
                "{rate_min}"
            );
        }
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

    /// **The design reports the buildable width, not only the bound.**
    ///
    /// `tapered_*` are the exact bounds and `built_*` are what a
    /// `cic_interp_tapered!` widget carries — the monotone lift, one bit
    /// more per stage boundary that dips. Reporting only the bound would
    /// under-state the hardware by a bit or two per stage, which is
    /// exactly the sort of drift a report exists to prevent.
    #[test]
    fn the_design_reports_both_the_bound_and_the_buildable_width() {
        let d = design(InterpSpec::default()).expect("designable");
        assert!(d.built_register_bits >= d.tapered_register_bits);
        assert!(d.built_register_bits < d.register_bits, "still a saving");
        for c in &d.cics {
            assert_eq!(c.built_widths.len(), c.stage_widths.len());
            for (built, exact) in c.built_widths.iter().zip(&c.stage_widths) {
                assert!(built >= exact, "the lift never narrows");
            }
            for pair in c.built_widths.windows(2) {
                assert!(pair[1] >= pair[0], "built widths are monotone");
            }
        }
    }

    /// **The design says how wide the compensator's output has to be.**
    ///
    /// The follow-up this closes: the designer used to report the
    /// compensator's gain and leave the headroom to the caller, so
    /// `CompensatedInterp`'s `W_MID` was a guess.
    #[test]
    fn the_design_reports_the_headroom_the_compensator_needs() {
        let d = design(InterpSpec::default()).expect("designable");
        // Both bounds are at least the input width -- a compensator
        // never needs *less* room than its input.
        assert!(d.mid_width_in_band >= d.spec.input_width);
        assert!(d.mid_width_any_input >= d.mid_width_in_band);
        // And the norms are consistent with the widths.
        assert!(d.compensator_l1 >= d.compensator_peak);
        assert!(
            d.compensator_peak >= 1.0,
            "a droop-inverting compensator has gain above one, got {}",
            d.compensator_peak
        );
    }

    /// **The two bounds differ, and by enough to matter.**
    ///
    /// If they were equal the distinction would be pedantry. The `l1`
    /// norm counts every tap's magnitude and the passband peak counts
    /// only what an in-band sinusoid sees, so a compensator with large
    /// alternating taps has a much bigger `l1` — and an envelope step,
    /// which every burst transmitter produces, is the input that reaches
    /// it.
    #[test]
    fn the_any_input_bound_is_wider_than_the_in_band_one() {
        let d = design(InterpSpec::default()).expect("designable");
        assert!(
            d.compensator_l1 > 1.5 * d.compensator_peak,
            "l1 {} against passband peak {}",
            d.compensator_l1,
            d.compensator_peak
        );
        assert!(
            d.mid_width_any_input > d.mid_width_in_band,
            "the two widths should differ: {} and {}",
            d.mid_width_any_input,
            d.mid_width_in_band
        );
    }

    /// The headroom function agrees with hand arithmetic on a filter
    /// whose norms are obvious.
    #[test]
    fn the_headroom_arithmetic_is_right() {
        // A unit impulse: both norms one, so no extra bits.
        let (any, band, l1, peak) = compensator_headroom_of(16, &[0.0, 1.0, 0.0], 0.4);
        assert_eq!((any, band), (16, 16));
        assert!((l1 - 1.0).abs() < 1e-12 && (peak - 1.0).abs() < 1e-12);

        // `[-1, 3, -1]` has response `3 - 2·cos(2πu)`, so `l1` is 5 and
        // the peak across a band reaching `u = 0.3` is `3 + 2·0.309`.
        let (any, band, l1, peak) = compensator_headroom_of(16, &[-1.0, 3.0, -1.0], 0.6);
        assert!((l1 - 5.0).abs() < 1e-12, "l1 {l1}");
        assert!((peak - 3.618).abs() < 0.01, "peak {peak}");
        assert_eq!(any, 16 + 3, "ceil(log2 5) = 3");
        assert_eq!(band, 16 + 2, "ceil(log2 3.618) = 2");

        // **A gain a hair above one costs a whole bit**, which is
        // arithmetically right and reads as a surprise: the same filter
        // over a band so narrow that its peak is 1.0005 still needs one
        // extra bit, because 1.0005 times full scale does not fit.
        // Pinned because an earlier version of this test expected zero.
        let (_, band, _, peak) = compensator_headroom_of(16, &[-1.0, 3.0, -1.0], 0.01);
        assert!(peak > 1.0 && peak < 1.001, "peak {peak}");
        assert_eq!(band, 17);
    }

    /// **A post-compensator suppresses images; a pre-compensator
    /// cannot.**
    ///
    /// The claim that justifies the whole converter-rate arrangement,
    /// measured on designed taps rather than argued from periodicity.
    /// The cascade alone rejects images by some amount; adding a
    /// *pre*-compensator leaves that number exactly where it was, and
    /// adding a *post*-compensator improves it.
    #[test]
    fn a_post_compensator_suppresses_images_and_a_pre_one_does_not() {
        let shapes = vec![compensator::CicShape {
            decimate: 4,
            stages: 2,
            delay: 1,
        }];
        let passband = 0.4;
        let r = 4usize;

        let bare = cascade_image_db(&shapes, passband, r).0;

        // A pre-compensator at the envelope rate.
        let pre = compensator::design(compensator::Spec {
            cics: shapes.clone(),
            passband,
            taps: 9,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
            max_ripple_db: 1.0,
            method: compensator::Method::LeastSquares,
        })
        .expect("designable");
        // Its gain is periodic, so the image-to-signal ratio is
        // unchanged -- which is what `cascade_image_db` already reports.
        let pre_edge = response::passband_edge_out(passband);
        let at_signal = compensator::fir_amplitude(&pre.taps, pre_edge).abs();
        let at_image = compensator::fir_amplitude(&pre.taps, 1.0 + pre_edge).abs();
        assert!(
            (at_signal - at_image).abs() < 1e-9 * at_signal.max(1.0),
            "a pre-compensator lifts signal and image alike"
        );

        // A post-compensator at the converter rate.
        let post =
            post_compensator(&shapes, passband, r, 25, 30.0, 18).expect("designable at R = 4");
        let scale = (1u64 << post.shift) as f64;
        let real: Vec<f64> = post.taps.iter().map(|x| *x as f64 / scale).collect();
        let improved = post_compensated_image_db(&shapes, passband, r, &real);

        assert!(
            improved > bare + 10.0,
            "a post-compensator must buy real rejection: bare {bare:.1} dB, \
             with the filter {improved:.1} dB"
        );
    }

    /// And it is refused, not fudged, where the bands do not fit.
    #[test]
    fn a_post_compensator_at_rate_one_is_refused() {
        let shapes = vec![compensator::CicShape {
            decimate: 4,
            stages: 2,
            delay: 1,
        }];
        // `R = 1` puts the stopband edge at or above Nyquist, so there is
        // no band to stop.
        assert!(post_compensator(&shapes, 0.4, 1, 25, 30.0, 18).is_none());
        // As is an even tap count.
        assert!(post_compensator(&shapes, 0.4, 4, 24, 30.0, 18).is_none());
    }

    /// **Remez is refused at a scale other than one**, rather than run
    /// with the wrong band bookkeeping.
    #[test]
    fn remez_is_refused_when_the_rates_differ() {
        let spec = compensator::Spec {
            cics: vec![compensator::CicShape {
                decimate: 8,
                stages: 3,
                delay: 1,
            }],
            passband: 0.1,
            taps: 15,
            stopband_edge: 0.4,
            min_stopband_db: 30.0,
            max_ripple_db: 1.0,
            method: compensator::Method::Remez,
        };
        assert!(compensator::design_scaled(spec.clone(), 8.0).is_none());
        // At scale one it is the ordinary designer and works.
        assert!(compensator::design_scaled(spec, 1.0).is_some());
    }

    /// **A post-compensator's tap count grows linearly with the rate.**
    ///
    /// The table in [`post_compensator_taps`]'s docs, pinned. This is the
    /// number that decides whether converter-rate compensation is worth
    /// considering, and the answer flips somewhere around `R = 8`.
    #[test]
    fn the_tap_count_grows_with_the_rate() {
        let at = |r: usize| post_compensator_taps(0.4, r, 60.0);
        for &(r, taps) in &[
            (2usize, 13usize),
            (4, 25),
            (8, 49),
            (16, 97),
            (32, 195),
            (125, 755),
        ] {
            assert_eq!(at(r), taps, "R={r}");
        }
        // Linear, not merely increasing: doubling the rate roughly
        // doubles the count, which is what makes the large-R case
        // hopeless rather than merely expensive.
        for r in [4usize, 8, 16, 32] {
            let ratio = at(2 * r) as f64 / at(r) as f64;
            assert!(
                (ratio - 2.0).abs() < 0.15,
                "R={r} -> {}: ratio {ratio:.3}",
                2 * r
            );
        }
        // And every count is odd, because a symmetric FIR needs a centre
        // tap.
        for r in [2usize, 3, 5, 8, 17, 125] {
            assert_eq!(post_compensator_taps(0.4, r, 60.0) % 2, 1, "R={r}");
        }
    }

    /// The bands are where a converter-rate filter sees them.
    #[test]
    fn the_post_compensator_bands_are_squeezed_by_the_rate() {
        let (p, s) = post_compensator_bands(0.4, 125);
        assert!((p - 0.2 / 125.0).abs() < 1e-12, "passband edge {p}");
        assert!((s - 0.8 / 125.0).abs() < 1e-12, "stopband edge {s}");
        // The signal band shrinks in proportion to R, which is the
        // mechanism behind the tap count.
        let (p2, _) = post_compensator_bands(0.4, 250);
        assert!((p2 - p / 2.0).abs() < 1e-12);
    }

    /// **Splitting the chain is what makes post-compensation affordable.**
    ///
    /// The practical reading of the tap table: after the first stage of a
    /// `5 × 25` split the local rate is five, not a hundred and
    /// twenty-five, so the filter is an order of magnitude shorter.
    #[test]
    fn post_compensating_between_stages_is_far_cheaper() {
        let whole = post_compensator_taps(0.4, 125, 60.0);
        let after_first = post_compensator_taps(0.4, 5, 60.0);
        assert!(
            after_first * 10 < whole,
            "after the first stage of 5 x 25: {after_first} taps against {whole}"
        );
    }

    /// **The runner-up is a different design from the winner.**
    ///
    /// It reported as identical until [`Alternative`] carried the
    /// differential delay: two candidates differing only in `M` are
    /// different filters, and the example's output printed
    /// "split [5, 25] N=[5, 2]" twice. A runner-up that looks the same
    /// as the winner is worse than no runner-up, because a reader
    /// concludes the search is broken.
    #[test]
    fn the_runner_up_is_a_different_design() {
        let d = design(InterpSpec::default()).expect("designable");
        let a = d.alternative.as_ref().expect("there is a runner-up here");
        assert!(
            a.split != d.split() || a.stages != d.depths() || a.delays != d.delays(),
            "runner-up {:?}/{:?}/{:?} is the winner {:?}/{:?}/{:?}",
            a.split,
            a.stages,
            a.delays,
            d.split(),
            d.depths(),
            d.delays()
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
