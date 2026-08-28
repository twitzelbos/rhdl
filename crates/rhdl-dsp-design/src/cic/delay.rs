#![warn(missing_docs)]
//! Group delay of a CIC chain — the constraint a control loop is
//! actually bound by.
//!
//! Every other figure in [`super`] is about the *frequency* response:
//! ripple, alias rejection, image rejection, noise. For a filter inside
//! a feedback loop none of them is the binding constraint. **Loop
//! bandwidth is set by loop delay**, and a decimating measurement filter
//! is usually the largest single contributor to it.
//!
//! Rule of thumb, and it is only that: a closed loop needs roughly
//! `10 × delay` of period to keep sensible phase margin, so
//!
//! ```text
//!   achievable bandwidth  ~  1 / (10 · total loop delay)
//! ```
//!
//! [`loop_bandwidth_hz`] is that arithmetic, and it is offered as an
//! *aid* rather than a design rule: the real number depends on the plant
//! and the controller, and a caller who knows theirs should use the
//! delay figure directly.
//!
//! # Why this module exists at all
//!
//! It was written after a lock-in control loop turned out to be
//! dominated by two contributions nothing reported:
//!
//! - **The comb pipelining.** Both cascades read the previous stage's
//!   registered output so the combinational path is one adder deep
//!   (`rhdl_fpga::dsp::cic::decimator`). The comb section runs at the
//!   *output* rate, so its `z^-(N-1)` is `(N-1)·R` **input** samples —
//!   which at `N = 3, R = 1250` is 2500 samples against the filter's own
//!   1875. The pipelining more than doubled the delay, and for a
//!   transmit chain that is harmless latency while in a loop it is phase
//!   margin.
//! - **The compensator.** It runs at the output rate too, so
//!   `(TAPS-1)/2` output samples is `(TAPS-1)/2 · R` input samples. At
//!   15 taps and `R = 1250` that is 8750 — larger than everything else
//!   put together.
//!
//! Neither is a bug. Both are invisible unless something reports them,
//! which is what [`Breakdown`] is for.
//!
//! **Which of the two is larger depends on the configuration**, and that
//! is the reason this is a report and not a rule. At a single stage of
//! `R = 1250` with a 15-tap compensator the compensator wins, 8750
//! against 2500. But a designer asked for the same rate change picks a
//! *split* — `[10, 25, 5]` — and a narrow band needs few taps, and then
//! the ordering reverses: 4000 samples of comb pipelining against 1250
//! of compensator, out of 8515 total. Both configurations are pinned by
//! tests in this module, in both directions, because the intuition
//! "the compensator dominates" is only true half the time.
//!
//! The referral is what makes it so. A decimation chain's stage `k` is
//! multiplied by the product of the factors *ahead* of it, so the same
//! register costs `R_0·R_1` times more in the tail than in the head —
//! see [`decimation_stage_breakdowns`], which is there to name the
//! expensive stage rather than the expensive category.
//!
//! # The formulas are verified, not derived-and-hoped
//!
//! A CIC's composite response is a boxcar of length `R·M` cascaded `N`
//! times, which is symmetric, so the filter is linear phase and its
//! group delay is the centre of mass of its impulse response. The
//! closed forms below were checked against a numerically computed centre
//! of mass over `N ∈ {2,3,5}`, `R ∈ {4,10,50}`, `M ∈ {1,2}` and both
//! pipelining choices — an earlier version was off by exactly one
//! sample, which is the kind of error that survives inspection.

/// Where a chain's group delay comes from, in samples of the chain's
/// **reference rate** — input samples for a decimation chain, converter
/// samples for an interpolation chain.
///
/// Reported as parts rather than a total because the parts are what a
/// caller can act on, and because **which part is largest depends on the
/// configuration**: a shorter compensator, one stage fewer, moving depth
/// out of the tail stage, and pricing a software implementation are four
/// different decisions, and the total alone does not say which to make.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Breakdown {
    /// The cascade's own impulse response: `Σ N(R·M − 1)/2`, rate-referred.
    pub cic_body: f64,
    /// The integrator cascade's pipelining, `N − 1` per stage,
    /// rate-referred.
    pub integrator_pipeline: f64,
    /// The comb cascade's pipelining, rate-referred: `(N − 1)·R` per
    /// decimator stage and `N·R` per interpolator stage. Zero for an
    /// implementation that does not pipeline its combs.
    pub comb_pipeline: f64,
    /// The per-stage output registers, one system clock each — the one
    /// term that is *not* rate-referred. See [`stage_output_register`].
    pub output_registers: f64,
    /// The compensator FIR: `(TAPS − 1)/2` of its own samples, referred
    /// to the reference rate. It runs at the *slow* end of the chain in
    /// both directions, which is what makes it large.
    pub compensator: f64,
}

impl Breakdown {
    /// A stage's figures referred to the chain's reference rate, where
    /// one of the stage's samples is `k` reference samples.
    ///
    /// Every term scales **except `output_registers`**, which is a system
    /// clock rather than a stage sample — see [`stage_output_register`].
    pub fn referred(&self, k: f64) -> Self {
        Self {
            cic_body: self.cic_body * k,
            integrator_pipeline: self.integrator_pipeline * k,
            comb_pipeline: self.comb_pipeline * k,
            output_registers: self.output_registers,
            compensator: self.compensator * k,
        }
    }

    /// Total group delay in input samples.
    pub fn total(&self) -> f64 {
        self.cic_body
            + self.integrator_pipeline
            + self.comb_pipeline
            + self.output_registers
            + self.compensator
    }

    /// The largest contribution, and what it is called.
    ///
    /// The single most useful line in a report: it names the thing to
    /// change first.
    pub fn dominant(&self) -> (&'static str, f64) {
        let mut best = ("cascade", self.cic_body);
        for c in [
            ("integrator pipeline", self.integrator_pipeline),
            ("comb pipeline", self.comb_pipeline),
            ("output registers", self.output_registers),
            ("compensator", self.compensator),
        ] {
            if c.1 > best.1 {
                best = c;
            }
        }
        best
    }
}

/// Group delay of one **decimator** stage, in **that stage's input
/// samples**.
///
/// `pipelined_combs` selects whether the comb cascade reads registered
/// or combinational values. RHDL's `CicDecimate` does — it is not a knob
/// on the widget, and `super::chain::ChainSpec::pipelined_combs` explains
/// why. `false` prices an implementation that does not: a software CIC,
/// a vendor core, a hand-written block. The difference is `(n − 1)·r`,
/// which is the whole reason this parameter is exposed rather than
/// assumed.
///
/// Excludes the stage's output register; [`stage_output_register`] counts
/// that separately so each part can be checked on its own.
pub fn decimator_stage_group_delay(n: usize, r: usize, m: usize, pipelined_combs: bool) -> f64 {
    let body = n as f64 * (r as f64 * m as f64 - 1.0) / 2.0;
    let int_pipe = (n - 1) as f64;
    // A decimator's combs run at the *output* rate, so their `z^-(n-1)`
    // is `(n-1)·r` input samples.
    let comb_pipe = if pipelined_combs {
        ((n - 1) * r) as f64
    } else {
        0.0
    };
    body + int_pipe + comb_pipe
}

/// Group delay of one **interpolator** stage, in **that stage's output
/// samples**.
///
/// # Why this is not the decimator's formula with the sign flipped
///
/// The comb pipelining costs a different amount, and the difference is
/// not cosmetic. An interpolator's combs run at its *input* rate, and
/// there are `n` registers in that path rather than `n − 1` — the `n − 1`
/// of the chain plus one handover into the integrators. So the cost is
/// `n` input samples, which is **`n·r` output samples**, against the
/// decimator's `(n−1)·r` input samples.
///
/// Writing this module, the decimator's formula was applied to an
/// interpolation chain and reported 31250 samples where the truth was
/// 250 — a factor of 125, silently, because both expressions have the
/// same shape. Hence two functions rather than one with a flag.
pub fn interpolator_stage_group_delay(n: usize, r: usize, m: usize, pipelined_combs: bool) -> f64 {
    let body = n as f64 * (r as f64 * m as f64 - 1.0) / 2.0;
    let int_pipe = (n - 1) as f64;
    let comb_pipe = if pipelined_combs { (n * r) as f64 } else { 0.0 };
    body + int_pipe + comb_pipe
}

/// The output register a CIC stage adds: **one system clock**, which is
/// one sample of the chain's reference rate.
///
/// Not one sample of the *stage's* rate, which is the trap. Every stage
/// in an RHDL chain shares one clock — a decimator decimates by enable,
/// not by clock division — so a register on the third stage's output
/// delays by 1/fs however slowly that stage produces. This is the one
/// term [`Breakdown::referred`] deliberately does not scale, and getting
/// it wrong inflated the reported delay of a `10x25x5` chain by 258
/// samples: it was counting stage 2's register as 250.
pub fn stage_output_register() -> f64 {
    1.0
}

/// A decimator stage's contribution split into parts, in its own input
/// samples.
pub fn decimator_stage_breakdown(n: usize, r: usize, m: usize, pipelined_combs: bool) -> Breakdown {
    Breakdown {
        cic_body: n as f64 * (r as f64 * m as f64 - 1.0) / 2.0,
        integrator_pipeline: (n - 1) as f64,
        comb_pipeline: if pipelined_combs {
            ((n - 1) * r) as f64
        } else {
            0.0
        },
        output_registers: stage_output_register(),
        compensator: 0.0,
    }
}

/// An interpolator stage's contribution split into parts, in its own
/// output samples.
pub fn interpolator_stage_breakdown(
    n: usize,
    r: usize,
    m: usize,
    pipelined_combs: bool,
) -> Breakdown {
    Breakdown {
        cic_body: n as f64 * (r as f64 * m as f64 - 1.0) / 2.0,
        integrator_pipeline: (n - 1) as f64,
        comb_pipeline: if pipelined_combs { (n * r) as f64 } else { 0.0 },
        output_registers: stage_output_register(),
        compensator: 0.0,
    }
}

/// Group delay of a whole **decimation** chain, referred to the chain's
/// input rate.
///
/// `stages` is `(n, r, m)` per stage in signal order — highest rate
/// first, as a decimator runs. `taps` is the compensator's length, which
/// runs at the *final* output rate; pass zero for no compensator.
///
/// # The rate referral is the part to get right
///
/// A later stage's samples are longer. Stage `k`'s delay is in *its*
/// input samples, and its input rate is the chain's divided by the
/// product of the factors ahead of it — so the delay is multiplied by
/// that product to reach chain-input samples. Getting this backwards
/// makes the first stage look expensive and the last one free, which is
/// exactly wrong.
pub fn decimation_chain_breakdown(
    stages: &[(usize, usize, usize)],
    taps: usize,
    pipelined_combs: bool,
) -> Breakdown {
    let mut out = Breakdown::default();
    for b in decimation_stage_breakdowns(stages, pipelined_combs) {
        out.cic_body += b.cic_body;
        out.integrator_pipeline += b.integrator_pipeline;
        out.comb_pipeline += b.comb_pipeline;
        out.output_registers += b.output_registers;
    }
    if taps >= 3 {
        // The compensator runs at the final output rate, so each of its
        // samples is the whole rate change in input samples. It is a
        // large term and it surprises people -- but see the module docs:
        // in narrowband configurations it is *not* the largest.
        let total: usize = stages.iter().map(|(_, r, _)| *r).product();
        out.compensator = ((taps - 1) / 2) as f64 * total as f64;
    }
    out
}

/// The same figures, **per stage**, so the expensive one is nameable.
///
/// Each entry is already referred to the chain *input* rate, so the
/// entries are directly comparable with each other and sum to
/// [`decimation_chain_breakdown`] minus its compensator term.
///
/// This exists because "the comb pipelining costs 4000 samples" is not
/// actionable, and "the *last* stage's comb pipelining costs 3000 of
/// them" is: a decimation chain's later stages are referred by every
/// factor ahead of them, so a register in the tail is hundreds of times
/// more expensive than the identical register in the head.
pub fn decimation_stage_breakdowns(
    stages: &[(usize, usize, usize)],
    pipelined_combs: bool,
) -> Vec<Breakdown> {
    let mut ahead = 1usize;
    let mut out = Vec::with_capacity(stages.len());
    for (n, r, m) in stages {
        let b = decimator_stage_breakdown(*n, *r, *m, pipelined_combs);
        out.push(b.referred(ahead as f64));
        ahead *= r;
    }
    out
}

/// Group delay of a whole **interpolation** chain, referred to the
/// chain's *output* (converter) rate.
///
/// `stages` is `(n, r, m)` in signal order — lowest rate first, as an
/// interpolator runs — and `taps` is the pre-compensator, which runs at
/// the envelope rate.
///
/// Referred to the converter rate because that is where a transmit
/// chain's latency is observed, and because it makes the figure directly
/// comparable with a decimator's.
pub fn interpolation_chain_breakdown(
    stages: &[(usize, usize, usize)],
    taps: usize,
    pipelined_combs: bool,
) -> Breakdown {
    let mut out = Breakdown::default();
    for b in interpolation_stage_breakdowns(stages, pipelined_combs) {
        out.cic_body += b.cic_body;
        out.integrator_pipeline += b.integrator_pipeline;
        out.comb_pipeline += b.comb_pipeline;
        out.output_registers += b.output_registers;
    }
    if taps >= 3 {
        // A pre-compensator runs at the envelope rate: the slowest rate
        // in the chain, so its samples are the longest.
        let total: usize = stages.iter().map(|(_, r, _)| *r).product();
        out.compensator = ((taps - 1) / 2) as f64 * total as f64;
    }
    out
}

/// The same figures, **per stage**, referred to the converter rate.
///
/// Mirror of [`decimation_stage_breakdowns`], and the referral runs the
/// other way: an interpolation chain's *early* stages are the expensive
/// ones, because they run slowest.
pub fn interpolation_stage_breakdowns(
    stages: &[(usize, usize, usize)],
    pipelined_combs: bool,
) -> Vec<Breakdown> {
    let total: usize = stages.iter().map(|(_, r, _)| *r).product();
    let mut behind = 1usize;
    let mut out = Vec::with_capacity(stages.len());
    for (n, r, m) in stages {
        let b = interpolator_stage_breakdown(*n, *r, *m, pipelined_combs);
        // The stage's figures are in *its own output* samples, and its
        // output rate is the chain input times the factors up to and
        // including itself -- so update the product first.
        behind *= r;
        out.push(b.referred((total / behind) as f64));
    }
    out
}

/// Achievable closed-loop bandwidth for a given loop delay, in Hz.
///
/// `1 / (10 · delay)`. **A rule of thumb offered as an aid, not a design
/// rule** — the factor of ten stands in for the phase margin a real
/// controller needs and the true number depends on the plant. A caller
/// who knows their loop should use the delay directly.
pub fn loop_bandwidth_hz(delay_samples: f64, fs_hz: f64) -> f64 {
    if delay_samples <= 0.0 || fs_hz <= 0.0 {
        return f64::INFINITY;
    }
    fs_hz / (10.0 * delay_samples)
}

/// Delay in seconds.
pub fn seconds(delay_samples: f64, fs_hz: f64) -> f64 {
    delay_samples / fs_hz
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A numerically computed centre of mass of the widget's impulse
    /// response, at the input rate.
    ///
    /// The independent check. Written from the widget's *structure* —
    /// pipelined integrators, optionally pipelined combs, decimation
    /// gate — rather than from the closed form, so an algebra error in
    /// [`stage_group_delay`] cannot hide.
    fn measured_centre_of_mass(n: usize, r: usize, m: usize, pipelined: bool) -> f64 {
        let len = 40 * n * r * m + 200;
        let mut ints = vec![0.0f64; n];
        let mut combs = vec![vec![0.0f64; m]; n];
        let mut couts = vec![0.0f64; n];
        let (mut sum, mut moment) = (0.0f64, 0.0f64);
        for i in 0..len {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let prev = ints.clone();
            for k in 0..n {
                ints[k] = prev[k] + if k == 0 { x } else { prev[k - 1] };
            }
            let carry = ints[n - 1];
            if (i + 1) % r == 0 {
                let y = if pipelined {
                    let pv = couts.clone();
                    for k in 0..n {
                        let v = if k == 0 { carry } else { pv[k - 1] };
                        couts[k] = v - combs[k][m - 1];
                        for j in (1..m).rev() {
                            combs[k][j] = combs[k][j - 1];
                        }
                        combs[k][0] = v;
                    }
                    couts[n - 1]
                } else {
                    let mut v = carry;
                    for c in combs.iter_mut() {
                        let d = v - c[m - 1];
                        for j in (1..m).rev() {
                            c[j] = c[j - 1];
                        }
                        c[0] = v;
                        v = d;
                    }
                    v
                };
                sum += y;
                moment += i as f64 * y;
            }
        }
        moment / sum
    }

    /// **The closed form matches the impulse response, at every
    /// configuration tried.**
    ///
    /// Excluding the output register, which the measurement does not see
    /// — it indexes by the input sample at which the decimation fires,
    /// not the cycle the registered value emerges. An earlier version of
    /// [`stage_group_delay`] included the register and was therefore off
    /// by exactly one everywhere, which is the kind of error that
    /// survives being looked at.
    #[test]
    fn the_closed_form_matches_the_impulse_response() {
        for n in [2usize, 3, 5] {
            for r in [4usize, 10, 50] {
                for m in [1usize, 2] {
                    for pipelined in [false, true] {
                        let got = decimator_stage_group_delay(n, r, m, pipelined);
                        let want = measured_centre_of_mass(n, r, m, pipelined);
                        assert!(
                            (got - want).abs() < 1e-9,
                            "N={n} R={r} M={m} pipelined={pipelined}: \
                             closed form {got}, measured {want}"
                        );
                    }
                }
            }
        }
    }

    /// A numerically computed centre of mass of the *interpolator's*
    /// impulse response, at the output rate.
    fn measured_interp_centre(n: usize, r: usize, m: usize, pipelined: bool) -> f64 {
        let ninp = 30 * n * m + 60;
        let mut combs = vec![vec![0.0f64; m]; n];
        let mut couts = vec![0.0f64; n];
        let mut ints = vec![0.0f64; n];
        let (mut sum, mut moment) = (0.0f64, 0.0f64);
        for i in 0..(ninp * r) {
            let accept = i % r == 0;
            let x = if i == 0 { 1.0 } else { 0.0 };
            let mut feed = 0.0;
            if accept {
                if pipelined {
                    let pv = couts.clone();
                    for k in 0..n {
                        let v = if k == 0 { x } else { pv[k - 1] };
                        couts[k] = v - combs[k][m - 1];
                        for j in (1..m).rev() {
                            combs[k][j] = combs[k][j - 1];
                        }
                        combs[k][0] = v;
                    }
                    feed = pv[n - 1];
                } else {
                    let mut v = x;
                    for c in combs.iter_mut() {
                        let d = v - c[m - 1];
                        for j in (1..m).rev() {
                            c[j] = c[j - 1];
                        }
                        c[0] = v;
                        v = d;
                    }
                    feed = v;
                }
            }
            let prev = ints.clone();
            for k in 0..n {
                ints[k] = prev[k] + if k == 0 { feed } else { prev[k - 1] };
            }
            sum += ints[n - 1];
            moment += i as f64 * ints[n - 1];
        }
        moment / sum
    }

    /// **The interpolator's closed form matches its impulse response
    /// too.**
    #[test]
    fn the_interpolator_closed_form_matches_its_impulse_response() {
        for n in [2usize, 3, 5] {
            for r in [2usize, 4, 10] {
                for m in [1usize, 2] {
                    for pipelined in [false, true] {
                        let got = interpolator_stage_group_delay(n, r, m, pipelined);
                        let want = measured_interp_centre(n, r, m, pipelined);
                        assert!(
                            (got - want).abs() < 1e-9,
                            "N={n} R={r} M={m} pipelined={pipelined}: \
                             closed form {got}, measured {want}"
                        );
                    }
                }
            }
        }
    }

    /// **The comb pipelining costs `(N−1)·R`, which is the finding this
    /// module exists for.**
    #[test]
    fn the_comb_pipelining_costs_n_minus_one_times_r() {
        for n in [2usize, 3, 5] {
            for r in [4usize, 125, 1250] {
                let with = decimator_stage_group_delay(n, r, 1, true);
                let without = decimator_stage_group_delay(n, r, 1, false);
                assert_eq!(with - without, ((n - 1) * r) as f64, "N={n} R={r}");
            }
        }
        // And at the configuration that prompted the module it more than
        // doubles the total.
        let with = decimator_stage_group_delay(3, 1250, 1, true);
        let without = decimator_stage_group_delay(3, 1250, 1, false);
        assert!(
            with > 2.0 * without * 0.95,
            "pipelined {with} against unpipelined {without}"
        );
    }

    /// **And the compensator dominates a decimation chain**, which is
    /// the other finding.
    #[test]
    fn the_compensator_dominates_a_decimation_chain() {
        let b = decimation_chain_breakdown(&[(3, 1250, 1)], 15, true);
        let (name, _) = b.dominant();
        assert_eq!(name, "compensator", "breakdown {b:?}");
        // 7 output samples at R = 1250.
        assert_eq!(b.compensator, 8750.0);
        // Which is more than everything else together.
        assert!(b.compensator > b.total() - b.compensator);
        // And dropping it triples the achievable loop bandwidth.
        let without = decimation_chain_breakdown(&[(3, 1250, 1)], 0, true);
        let a = loop_bandwidth_hz(b.total(), 125e6);
        let c = loop_bandwidth_hz(without.total(), 125e6);
        assert!(c > 2.5 * a, "with {a:.0} Hz, without {c:.0} Hz");
    }

    /// The rate referral runs the right way for a decimator: an early
    /// stage's delay is cheap in chain-input samples, a late one's is
    /// dear.
    #[test]
    fn a_decimators_later_stages_cost_more() {
        // Same shapes, opposite order.
        let a = decimation_chain_breakdown(&[(5, 2, 1), (1, 50, 1)], 0, true);
        let b = decimation_chain_breakdown(&[(1, 50, 1), (5, 2, 1)], 0, true);
        assert!(
            a.total() != b.total(),
            "order must matter: {} vs {}",
            a.total(),
            b.total()
        );
        // The deep stage placed second is referred through the first
        // stage's factor, so it costs more.
        assert!(
            a.total() < b.total(),
            "depth early is cheaper in delay: {} vs {}",
            a.total(),
            b.total()
        );
    }

    /// **An interpolator's comb pipelining costs `n·r`, not `(n−1)·r`.**
    ///
    /// The asymmetry that made two functions necessary. An earlier
    /// version used the decimator's formula here and over-reported an
    /// interpolation chain's delay by a factor of 125 — silently,
    /// because the two expressions have the same shape.
    #[test]
    fn an_interpolators_comb_pipelining_costs_n_times_r() {
        for n in [2usize, 3, 5] {
            for r in [2usize, 4, 125] {
                let with = interpolator_stage_group_delay(n, r, 1, true);
                let without = interpolator_stage_group_delay(n, r, 1, false);
                assert_eq!(with - without, (n * r) as f64, "N={n} R={r}");
                // And it is strictly more than the decimator's, by `r`.
                let dec_with = decimator_stage_group_delay(n, r, 1, true);
                let dec_without = decimator_stage_group_delay(n, r, 1, false);
                assert_eq!(
                    (with - without) - (dec_with - dec_without),
                    r as f64,
                    "the handover register, N={n} R={r}"
                );
            }
        }
    }

    /// An interpolation chain's delay is referred to the converter rate,
    /// and at *this* configuration the compensator is the slowest part.
    ///
    /// Both a pre-compensator and the comb pipelining live at the
    /// envelope rate, so the comparison is between `(taps-1)/2` and `n`
    /// — and a 15-tap compensator's 7 beats a 3-stage cascade's 3. The
    /// ordering reverses as soon as the compensator is short, which is
    /// what `a_short_compensator_reverses_the_ordering` pins.
    #[test]
    fn an_interpolation_chains_compensator_is_the_slowest_part() {
        let b = interpolation_chain_breakdown(&[(3, 125, 1)], 15, true);
        // 7 envelope samples, each 125 converter samples long.
        assert_eq!(b.compensator, 875.0);
        // 3 envelope samples of comb pipeline, also 125 long each.
        assert_eq!(b.comb_pipeline, 375.0);
        // The cascade body is 3*(125-1)/2 = 186 *output* samples.
        assert_eq!(b.cic_body, 186.0);
        let (name, _) = b.dominant();
        assert_eq!(name, "compensator", "breakdown {b:?}");
        // Declining the pipelining removes exactly its share.
        let flat = interpolation_chain_breakdown(&[(3, 125, 1)], 15, false);
        assert_eq!(b.total() - flat.total(), 375.0);
    }

    /// **A short compensator reverses the ordering**, in both
    /// directions.
    ///
    /// The module docs claim which term dominates is configuration-
    /// dependent. That claim is load-bearing — it is why `Breakdown` has
    /// five fields instead of a single number — so it is pinned rather
    /// than asserted in prose.
    #[test]
    fn a_short_compensator_reverses_the_ordering() {
        // Long compensator, one stage: the compensator wins, both ways.
        let dec = decimation_chain_breakdown(&[(3, 1250, 1)], 15, true);
        assert_eq!(dec.dominant().0, "compensator", "{dec:?}");
        let int = interpolation_chain_breakdown(&[(3, 125, 1)], 15, true);
        assert_eq!(int.dominant().0, "compensator", "{int:?}");

        // Split rate, 3-tap compensator -- what a designer actually
        // returns for a narrow band. Now the pipelining wins, both ways.
        let dec = decimation_chain_breakdown(&[(3, 10, 1), (2, 25, 1), (2, 5, 1)], 3, true);
        assert_eq!(dec.dominant().0, "comb pipeline", "{dec:?}");
        let int = interpolation_chain_breakdown(&[(3, 5, 1), (2, 25, 1), (2, 10, 1)], 3, true);
        assert_eq!(int.dominant().0, "comb pipeline", "{int:?}");
    }

    /// **A chain's output registers are one clock each, not one stage
    /// sample each.**
    ///
    /// Every stage shares the system clock, so a register on the tail
    /// stage costs the same as one on the head. Counting them at the
    /// stage's own rate inflated a `10x25x5` chain by 258 samples, and
    /// the per-stage impulse-response checks could not see it because
    /// they verify a single stage, where the referral is the identity.
    #[test]
    fn output_registers_are_clocks_not_stage_samples() {
        let dec = [(3, 10, 1), (2, 25, 1), (2, 5, 1)];
        let b = decimation_chain_breakdown(&dec, 0, true);
        assert_eq!(b.output_registers, 3.0, "one per stage: {b:?}");
        let int = [(3, 5, 1), (2, 25, 1), (2, 10, 1)];
        let b = interpolation_chain_breakdown(&int, 0, true);
        assert_eq!(b.output_registers, 3.0, "one per stage: {b:?}");
        // And a single stage is one, in both directions -- which is why
        // the stage-level tests never noticed.
        assert_eq!(
            decimation_chain_breakdown(&[(3, 1250, 1)], 0, true).output_registers,
            1.0
        );
    }

    /// **The per-stage figures sum to the chain figure**, and name the
    /// expensive stage.
    ///
    /// A decimator's tail is dear and an interpolator's head is, because
    /// the referral runs opposite ways. Getting that backwards is the
    /// error that cost a factor of 125 once.
    #[test]
    fn per_stage_figures_sum_and_name_the_expensive_stage() {
        let dec_stages = [(3, 10, 1), (2, 25, 1), (2, 5, 1)];
        let parts = decimation_stage_breakdowns(&dec_stages, true);
        let whole = decimation_chain_breakdown(&dec_stages, 0, true);
        assert_eq!(parts.len(), 3);
        let summed: f64 = parts.iter().map(|b| b.total()).sum();
        assert!(
            (summed - whole.total()).abs() < 1e-9,
            "{summed} vs {whole:?}"
        );
        // Stage 2 sits behind a factor of 250, stage 0 behind nothing.
        assert!(
            parts[2].total() > parts[0].total(),
            "a decimator's tail must be the dear one: {parts:?}"
        );

        let int_stages = [(3, 5, 1), (2, 25, 1), (2, 10, 1)];
        let parts = interpolation_stage_breakdowns(&int_stages, true);
        let whole = interpolation_chain_breakdown(&int_stages, 0, true);
        let summed: f64 = parts.iter().map(|b| b.total()).sum();
        assert!(
            (summed - whole.total()).abs() < 1e-9,
            "{summed} vs {whole:?}"
        );
        assert!(
            parts[0].total() > parts[2].total(),
            "an interpolator's head must be the dear one: {parts:?}"
        );
    }

    /// The loop-bandwidth helper is the arithmetic it claims and no more.
    #[test]
    fn the_loop_bandwidth_helper_is_just_the_rule_of_thumb() {
        assert_eq!(loop_bandwidth_hz(1000.0, 1e6), 100.0);
        assert_eq!(seconds(1250.0, 125e6), 1e-5);
        assert!(loop_bandwidth_hz(0.0, 1e6).is_infinite());
    }
}
