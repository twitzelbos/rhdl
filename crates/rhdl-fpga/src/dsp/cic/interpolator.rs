#![warn(missing_docs)]
//! `CicInterpolate` — a CIC interpolator with a run-time-variable rate.
//!
//! The transmit-side counterpart of [`super::decimator::CicDecimate`],
//! and structurally its transpose: comb section first at the low input
//! rate, then zero-stuffing, then the integrator cascade at the high
//! output rate. Same `sinc^N` shape, same absence of multipliers, same
//! `(R·M)`-spaced nulls — which on this side of the radio suppress the
//! *images* the upsampling would otherwise leave in the band.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+CicInterpolate+---------+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
+---->+ rate                     |
      |   Bits<CW>        sample |
+---->+ restart    SignedBits<WA>+----->
      |                          |
+---->+ downstream_ready         |
      |             input_ready  +----->
      |                  starved +----->
      |                  overrun +----->
      +--------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!(r"
  in ->[C0]->[C1]->..[Cn]-+          +->[I0]->[I1]->..[In]-> out
       combs, once per R   |  x R     |  integrators, every
       (y = x - x[-M])     +--stuff---+  output cycle
                              0,0,..
")]
//!
//! # The rate is an input, and that is cheap here
//!
//! `R` appears in exactly one place: the phase counter that decides
//! which output cycle takes an input. Nothing else in the datapath
//! knows the rate, so making it an input costs a comparator against
//! [`In::rate`] instead of against a constant.
//!
//! What it costs *elsewhere* is honesty about two things. The widths
//! must be sized for `R_MAX`, which they are and which
//! [`Default`] checks. And the gain varies with the rate — see below.
//!
//! A rate of zero or one makes every cycle an input cycle, which is
//! `R = 1` and a pass-through. The comparison is `phase + 1 >= rate`
//! rather than `==`, so lowering [`In::rate`] mid-count wraps the
//! counter immediately instead of running it all the way round.
//!
//! # The gain is `(R·M)^N / R`, and it moves when the rate does
//!
//! Not `(R·M)^N`. The transfer function's DC gain is that, but
//! zero-stuffing divides the signal by `R` on the way in, so the factor
//! a caller has to undo is
//! [`super::interp::dc_gain_ratio`] — exposed as an exact ratio, and
//! deliberately not applied, for the same reason the decimator does not
//! normalise: where to rescale depends on what comes next.
//!
//! **With a variable rate that factor is a run-time quantity.** A
//! downstream stage that scales must scale by the rate it set. This is
//! the real cost of the variable rate and it is not hidden.
//!
//! ## Change the rate with a restart, or the level will not move
//!
//! **A rate change alone does not rescale what the integrators are
//! already holding**, and on a slowly-varying signal that is very
//! visible: the output keeps the *old* rate's amplitude.
//!
//! The reason is the structure, not an oversight. The integrators only
//! move when the comb section feeds them, and the comb section's output
//! is the `N`-th difference of the input — zero for a constant. So on a
//! steady envelope there is nothing arriving to re-establish the level,
//! and the integrators sit where the previous rate left them. Measured
//! at `N = 2`: a constant 5 settles at 20 with `R = 4`, and switching
//! to `R = 8` leaves it at 20 rather than moving to 40.
//!
//! So **assert [`In::restart`] on the first sample at a new rate.** The
//! restart clears the cascade and the new gain establishes itself from
//! a clean window. `changing_the_rate_alone_leaves_the_level_stuck` and
//! `a_restart_makes_the_new_rate_take_effect` are the two halves of
//! this, so the behaviour cannot drift silently.
//!
//! # Both cascades are one adder deep, and what that costs
//!
//! Combinational depth does not care how often the logic is used. A
//! cascade chained combinationally has to settle inside one clock period
//! whether its registers move every cycle or one cycle in `R`, so both
//! sections here read the previous stage's *registered* value:
//!
//! | section | depth between registers | clocked |
//! |---|---|---|
//! | comb | 1 subtractor | one cycle in `R` |
//! | integrator | 1 adder | every cycle |
//!
//! Chained, the comb section would be `STAGES` subtractors — five 21-bit
//! subtractors in an 8 ns budget at `N = 5, R = 125`, which is the path
//! that would have set fmax.
//!
//! **The group delay is `STAGES` input samples plus `STAGES - 1` output
//! cycles.** The comb chain contributes `STAGES - 1` input samples, the
//! handover into the integrators one more, and the integrator chain
//! `STAGES - 1` output cycles. At the input rate that is `STAGES · R`
//! output cycles, which dominates: 375 cycles at `N = 3, R = 125`. All of
//! it is delay rather than distortion — the transfer function gains
//! `z^-k` factors whose magnitude is one — but a phase-sensitive
//! transmitter has to account for it, and
//! [`super::interp_stream`] says where the framing mark sits relative to
//! it.
//!
//! [`super::delay`] has the arithmetic for a whole chain, including the
//! rate referral, and a breakdown that names which term dominates —
//! which for a transmit chain is often this one rather than the
//! compensator.
//!
//! ## The pipelining and the run-time rate
//!
//! They interact in exactly one place, and it is the reason the comb
//! cascade had to be pipelined rather than left alone.
//!
//! **Correctness: no interaction.** The comb section computes
//! `(1 - z^-M)^N` at the input rate, which has no `R` in it, and its
//! pipeline advances on accept *events* rather than on cycles — so the
//! depth is `STAGES` accepts whatever the rate. A rate change cannot
//! corrupt values in flight because they never depended on the rate.
//! [`In::restart`] clears the output registers along with the delay
//! lines.
//!
//! **Latency: yes, proportionally.** `STAGES · R` output cycles, so the
//! group delay moves when the rate does — measured at 7, 10, 16, 28 and
//! 52 cycles for `R` of 1, 2, 4, 8 and 16 at `N = 3`, linear with slope
//! `N` and a rate-independent intercept.
//!
//! **And the critical path is now rate-independent, which it was not
//! before.** Accepts arrive every `R` cycles, so at `R = 1` the comb
//! logic runs at the full converter clock. Unpipelined, its critical
//! path would have been `STAGES` chained subtractors *whose timing
//! depended on a run-time input* — and since the rate can drop to one,
//! timing would have had to close for that case anyway, so the combs
//! being idle most of the time bought nothing. There is no way to close
//! timing against a value the tool cannot see. `tests/interp_variable_rate.rs`
//! is the evidence for all three.
//!
//! # Widths taper losslessly, and pruning does not apply
//!
//! [`super::interp`] carries the analysis. Two results matter to a
//! reader of this file:
//!
//! - Every stage has its own exact gain bound `G_j`, so a tapered
//!   interpolator is **bit-identical** to a uniform-width one. Unlike
//!   the decimator's Hogenauer schedule, tapering an interpolator costs
//!   no noise at all.
//! - Hogenauer's §V pruning does **not** transfer. Truncating anywhere
//!   ahead of an integrator feeds a `-1/2` LSB bias into a pole at DC,
//!   which grows without bound. The only place this structure may
//!   truncate is after its final integrator.
//!
//! This widget is the uniform-width form: every stage at
//! [`super::interp::accumulator_width`] for `R_MAX`. It is exact, and
//! it is the reference a tapered version is checked against.
//!
//! # Starvation feeds zero, and says so
//!
//! An interpolator emits on *every* output cycle — that is what it is
//! for — so its output is a plain value rather than an `Option`. An
//! `Option` that is always `Some` would be a lie with a wire attached.
//!
//! That makes the interesting failure the other one: an input cycle
//! arrives and upstream has nothing. [`Out::input_ready`] is the
//! widget asking, one cycle at a time, and a `None` on an input cycle
//! is reported by [`Out::starved`].
//!
//! **The starved cycle feeds zero, not the previous sample.** Zero is
//! the correct choice and holding is not. A permanently zero input
//! drives the comb section to zero and the composite response is FIR,
//! so the output decays to silence — which is what "the transmitter
//! stopped" should sound like. Holding the last sample instead makes
//! the comb output zero while the integrators keep whatever they held,
//! leaving a stuck DC offset on the DAC forever.
//!
//! # This widget does not stall
//!
//! The integrator cascade is tied to the output clock grid: pausing it
//! does not delay the signal, it corrupts the interpolation phase. A
//! low `downstream_ready` therefore loses that output sample, and
//! [`Out::overrun`] reports it rather than hiding it. Because output is
//! produced every cycle, `overrun` is simply `!downstream_ready` — a
//! DUC feeding a DAC should hold it high permanently.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_interpolate.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_interpolate.md")]

use rhdl::prelude::*;

use super::interp;
use crate::core::dff;
use crate::dsp::sign_extend;

/// An `N`-stage CIC interpolator with a run-time-variable rate.
///
/// `R_MAX` is the largest rate [`In::rate`] may carry; every width is
/// sized for it. See the module docs for what each parameter means.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CicInterpolate<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R_MAX: usize,
    const M: usize,
    const CW: usize,
> where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// Comb delay lines, `M` deep per stage, at the **input** rate.
    combs: dff::DFF<[[SignedBits<W_ACC>; M]; STAGES]>,
    /// Each comb stage's registered output.
    ///
    /// What makes the comb cascade one subtractor deep instead of
    /// `STAGES` — see the kernel. Clocked at the input rate, so it costs
    /// `STAGES · W_ACC` bits that move one cycle in `R`.
    comb_out: dff::DFF<[SignedBits<W_ACC>; STAGES]>,
    /// Running sums, one per stage, at the **output** rate.
    integrators: dff::DFF<[SignedBits<W_ACC>; STAGES]>,
    /// Counts output cycles since the last input was taken.
    phase: dff::DFF<Bits<CW>>,
    /// The interpolated result, registered.
    out: dff::DFF<SignedBits<W_ACC>>,
    /// An input cycle found nothing on the input.
    starved: dff::DFF<bool>,
}

/// Inputs to [`CicInterpolate`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W_IN: usize, const CW: usize>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    /// The next low-rate sample, consumed on a cycle where
    /// [`Out::input_ready`] is high.
    ///
    /// Presenting one on a cycle that is not an input cycle is not an
    /// error and not a stall — it is ignored, per the `RCStream`
    /// contract, and upstream is expected to hold it. `None` on an
    /// input cycle is starvation; see [`Out::starved`].
    pub sample: Option<SignedBits<W_IN>>,
    /// The interpolation factor, `R`.
    ///
    /// Carries `R` as a value, so `CW` is
    /// [`super::interp::rate_width`] wide rather than
    /// [`super::interp::counter_width`] — one bit more at the powers of
    /// two.
    ///
    /// Sampled on the cycle the phase counter wraps, so changing it
    /// takes effect from the next input onward rather than mid-window.
    /// Zero and one both mean `R = 1`.
    pub rate: Bits<CW>,
    /// **Restart the interpolation grid on this sample.**
    ///
    /// Clears the comb lines and the integrators and makes this cycle
    /// input number zero of a fresh window, wherever the phase counter
    /// happened to be. The transmit counterpart of the decimator's
    /// restart, and the same reasoning: an `N`-stage cascade's window
    /// is `N·R·M` output samples long, so realigning the grid without
    /// clearing the state would leak the previous burst into the start
    /// of this one.
    pub restart: bool,
    /// Downstream's ready, per the `RCStream` contract.
    ///
    /// **This widget does not stall** — see the module docs. A low
    /// `ready` loses that output cycle, which [`Out::overrun`] reports.
    pub downstream_ready: bool,
}

/// Outputs from [`CicInterpolate`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W_ACC: usize>
where
    rhdl::bits::W<W_ACC>: BitWidth,
{
    /// The interpolated sample, present on **every** cycle.
    ///
    /// A plain value rather than an `Option`, because an interpolator
    /// produces output continuously; see the module docs. Carries the
    /// full `(R·M)^N / R` gain, which varies with the rate.
    pub sample: SignedBits<W_ACC>,
    /// **This cycle takes an input.**
    ///
    /// The upstream-facing ready. Depends only on the registered phase
    /// counter — not on [`In::sample`] and not on
    /// [`In::downstream_ready`] — so it satisfies the `RCStream`
    /// requirement that data must not depend combinationally on ready.
    pub input_ready: bool,
    /// An input cycle found `None` and fed zero instead.
    ///
    /// Registered, so it is asserted alongside the first output that
    /// the missing sample contributed to.
    pub starved: bool,
    /// The output sample was produced while `downstream_ready` was low,
    /// and is gone.
    pub overrun: bool,
    /// The result was clipped to fit the output width.
    ///
    /// **Always false for this widget.** Every stage is at the exact
    /// growth bound for `R_MAX`, so no value can exceed its register.
    /// The field exists so that a compensated interpolator — which has
    /// gain above one and therefore can clip — presents this same
    /// interface and drops into any slot that takes one.
    pub saturated: bool,
}

impl<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R_MAX: usize,
    const M: usize,
    const CW: usize,
> Default for CicInterpolate<W_IN, W_ACC, STAGES, R_MAX, M, CW>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    fn default() -> Self {
        // Checked, not trusted -- and the consequence of getting it
        // wrong is worse here than in a decimator. A decimator's
        // integrators wrap and its combs undo the wrap; an
        // interpolator's last integrator *is* the output, so a wrap
        // there is a wrap in the answer.
        assert!(
            interp::accumulator_width_is_sufficient(W_IN, W_ACC, STAGES, R_MAX, M),
            "W_ACC must be at least W_IN + interp::gain_bits(STAGES, R_MAX, M); \
             an interpolator's final integrator is its output, so a wrap is not \
             cancelled downstream"
        );
        // `rate_width`, not `counter_width`: `In::rate` carries `R`
        // itself, which at a power of two needs one bit more than
        // counting to it does. `bits::<3>(8)` does not exist.
        assert!(
            CW >= interp::rate_width(R_MAX),
            "CW must be wide enough to carry R_MAX as a value, which is one              bit more than counting to it when R_MAX is a power of two"
        );
        assert!(
            R_MAX >= 2,
            "an interpolation factor below two is not an interpolator"
        );
        assert!(M >= 1, "the differential delay must be at least one");
        Self {
            combs: dff::DFF::new([[SignedBits::<W_ACC>::default(); M]; STAGES]),
            comb_out: dff::DFF::new([SignedBits::<W_ACC>::default(); STAGES]),
            integrators: dff::DFF::new([SignedBits::<W_ACC>::default(); STAGES]),
            phase: dff::DFF::new(bits::<CW>(0)),
            out: dff::DFF::new(SignedBits::<W_ACC>::default()),
            starved: dff::DFF::new(false),
        }
    }
}

impl<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R_MAX: usize,
    const M: usize,
    const CW: usize,
> SynchronousIO for CicInterpolate<W_IN, W_ACC, STAGES, R_MAX, M, CW>
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    type I = In<W_IN, CW>;
    type O = Out<W_ACC>;
    type Kernel = cic_interpolate_kernel<W_IN, W_ACC, STAGES, R_MAX, M, CW>;
}

#[kernel]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn cic_interpolate_kernel<
    const W_IN: usize,
    const W_ACC: usize,
    const STAGES: usize,
    const R_MAX: usize,
    const M: usize,
    const CW: usize,
>(
    cr: ClockReset,
    i: In<W_IN, CW>,
    q: Q<W_IN, W_ACC, STAGES, R_MAX, M, CW>,
) -> (Out<W_ACC>, D<W_IN, W_ACC, STAGES, R_MAX, M, CW>)
where
    rhdl::bits::W<W_IN>: BitWidth,
    rhdl::bits::W<W_ACC>: BitWidth,
    rhdl::bits::W<CW>: BitWidth,
{
    let mut d = D::<W_IN, W_ACC, STAGES, R_MAX, M, CW>::dont_care();

    // Hold the comb section by default; it only moves on an input
    // cycle. The integrators are assigned unconditionally below --
    // they move on every cycle, which is the whole point of the
    // structure.
    d.combs = q.combs;
    d.comb_out = q.comb_out;
    d.phase = q.phase;

    // ---- the phase counter, which is where the rate lives ----
    //
    // An input is taken when the counter is at zero, or when the caller
    // forces one with `restart`.
    let at_zero = q.phase == bits::<CW>(0);
    let take = at_zero || i.restart;

    // `+ 1 >= rate` rather than `== rate - 1`. Two reasons: `rate = 0`
    // would underflow the subtraction, and lowering the rate mid-count
    // must wrap the counter now rather than after a full lap of the old
    // rate.
    let phase_now = if i.restart { bits::<CW>(0) } else { q.phase };
    let wrap = (phase_now + bits::<CW>(1)) >= i.rate;
    d.phase = if wrap {
        bits::<CW>(0)
    } else {
        phase_now + bits::<CW>(1)
    };

    // ---- the input, or zero ----
    let mut starved_now = false;
    let mut x = signed::<W_ACC>(0);
    if take {
        match i.sample {
            Some(s) => {
                // `sign_extend`, not `resize`: `s` is unwrapped from an
                // `Option`, and `resize` on such a value zero-extends in
                // the emitted Verilog while the Rust simulator
                // sign-extends. See `crate::dsp::sign_extend`.
                x = sign_extend::<W_IN, W_ACC>(s);
            }
            None => {
                // Zero, not the previous sample -- see the module docs
                // on why holding leaves a stuck DC offset.
                starved_now = true;
            }
        }
    }
    d.starved = starved_now;

    // ---- comb cascade, once per input ----
    //
    // Runs at the input rate, so it is clocked one cycle in `R`.
    //
    // *** Pipelined: each stage reads the previous stage's REGISTERED
    // output, not its combinational one. ***
    //
    // Combinational depth does not care how often the logic is used. A
    // chained cascade is `STAGES` subtractors between registers and has
    // to settle inside one clock period regardless of the rate the
    // registers move at, so at `N = 5` it was five 21-bit subtractors in
    // an 8 ns budget. Reading the registered value puts exactly one
    // subtractor between registers however deep the cascade.
    //
    // The cost is latency, not response: it multiplies the transfer
    // function by `z^-STAGES` at the *input* rate -- `STAGES - 1` for the
    // chain and one more for the handover into the integrators -- whose
    // magnitude is one. At the input rate, so `STAGES · R` output cycles;
    // see the module docs on group delay.
    let mut feed = signed::<W_ACC>(0);
    if take {
        let prior_combs = if i.restart {
            [[signed::<W_ACC>(0); M]; STAGES]
        } else {
            q.combs
        };
        let prior_out = if i.restart {
            [signed::<W_ACC>(0); STAGES]
        } else {
            q.comb_out
        };
        let mut cs = prior_combs;
        let mut outs = prior_out;
        for k in 0..STAGES {
            // Stage one takes the sample; every later stage takes the
            // previous stage's *registered* difference.
            let v = if k == 0 { x } else { prior_out[k - 1] };
            // y = x - x[-M]; then shift this stage's delay line.
            let delayed = prior_combs[k][M - 1];
            outs[k] = v - delayed;
            let mut line = prior_combs[k];
            for j in 0..M {
                // Shift toward the tail, newest at index 0.
                let idx = M - 1 - j;
                line[idx] = if idx == 0 { v } else { prior_combs[k][idx - 1] };
            }
            cs[k] = line;
        }
        d.combs = cs;
        d.comb_out = outs;
        // The *registered* last stage, so the integrator's adder is one
        // deep from a register rather than one subtractor plus one adder.
        feed = prior_out[STAGES - 1];
    }

    // ---- integrator cascade, every output cycle ----
    //
    // *** Pipelined: each stage reads the previous stage's REGISTERED
    // output, not its combinational one. ***
    //
    // This section runs at the full output rate -- 125 MHz in the DUC
    // this was written for -- and is the widget's critical path.
    // Chaining the new values would make it `STAGES` adders deep in one
    // cycle and would set fmax. Reading the registered value puts
    // exactly one adder between registers regardless of depth.
    //
    // The cost is latency, not response: it multiplies the transfer
    // function by `z^-(STAGES-1)`, whose magnitude is one.
    //
    // `feed` is zero on every cycle that did not take an input, which
    // *is* the zero-stuffing -- there is no separate upsampler.
    let mut ints = q.integrators;
    for k in 0..STAGES {
        let prior = if i.restart {
            signed::<W_ACC>(0)
        } else {
            q.integrators[k]
        };
        let inp = if k == 0 {
            feed
        } else if i.restart {
            signed::<W_ACC>(0)
        } else {
            q.integrators[k - 1]
        };
        // Wraps, and for a correctly sized `W_ACC` never does: every
        // stage is at its own growth bound.
        ints[k] = prior + inp;
    }
    d.integrators = ints;
    d.out = ints[STAGES - 1];

    let mut o = Out::<W_ACC> {
        sample: q.out,
        // Only the counter, not `restart`: `restart` is the caller
        // forcing an input in, not the widget asking for one.
        input_ready: at_zero,
        starved: q.starved,
        // Output every cycle, so any low `ready` loses a sample.
        overrun: !i.downstream_ready,
        // Exact at this width -- see `Out::saturated`.
        saturated: false,
    };

    if cr.reset.any() {
        d.combs = [[signed::<W_ACC>(0); M]; STAGES];
        d.comb_out = [signed::<W_ACC>(0); STAGES];
        d.integrators = [signed::<W_ACC>(0); STAGES];
        d.phase = bits::<CW>(0);
        d.out = signed::<W_ACC>(0);
        d.starved = false;
        o.sample = signed::<W_ACC>(0);
        o.input_ready = false;
        o.starved = false;
        o.overrun = false;
        o.saturated = false;
    }

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    /// A small validated configuration: 8-bit input, two stages, rates
    /// up to eight.
    const WI: usize = 8;
    const WA: usize = 11;
    const S: usize = 2;
    const RMAX: usize = 8;
    const M: usize = 1;
    const CW: usize = 4;
    type Uut = CicInterpolate<WI, WA, S, RMAX, M, CW>;

    /// An independent software interpolator, written from the
    /// definition rather than from the widget.
    ///
    /// Structured differently on purpose: the comb section runs to
    /// completion over the whole low-rate input before the integrator
    /// loop starts, where the widget interleaves them one cycle at a
    /// time. A transcription error in the widget's cascade would have
    /// to be reproduced here in a different shape to go unnoticed.
    fn model(x: &[i128], stages: usize, r: usize, m: usize) -> Vec<i128> {
        // `(1 - z^-M)^N · z^-N` at the input rate.
        //
        // Pipelined, matching the widget: each stage reads the previous
        // stage's value from the *previous* input sample, and the
        // integrators are handed the last stage's previous value too.
        // That is where the `z^-N` comes from -- `N-1` for the chain and
        // one for the handover.
        let mut lines = vec![vec![0i128; m]; stages];
        let mut outs = vec![0i128; stages];
        let mut combed = Vec::new();
        for s in x {
            let prev = outs.clone();
            // What the integrators receive on this input cycle.
            combed.push(prev[stages - 1]);
            for k in 0..stages {
                let v = if k == 0 { *s } else { prev[k - 1] };
                outs[k] = v - lines[k][m - 1];
                for j in (1..m).rev() {
                    lines[k][j] = lines[k][j - 1];
                }
                lines[k][0] = v;
            }
        }
        // Zero-stuff by R and integrate at the output rate, pipelined:
        // stage k reads stage k-1's value from the previous cycle.
        let mut ints = vec![0i128; stages];
        let mut out = Vec::new();
        for n in 0..(combed.len() * r) {
            let feed = if n % r == 0 { combed[n / r] } else { 0 };
            let prev = ints.clone();
            for (k, i) in ints.iter_mut().enumerate() {
                *i += if k == 0 { feed } else { prev[k - 1] };
            }
            out.push(ints[stages - 1]);
        }
        out
    }

    /// Present `x[n / rate]` on every cycle.
    ///
    /// Deliberately every cycle rather than only on the input cycles:
    /// the widget must ignore a sample offered when it is not asking,
    /// and driving it constantly is the cheapest way to check that.
    fn stimulus(x: &[i128], rate: usize, drain: usize) -> Vec<In<WI, CW>> {
        let mut seq: Vec<In<WI, CW>> = (0..x.len() * rate)
            .map(|n| In::<WI, CW> {
                sample: Some(signed::<WI>(x[n / rate])),
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        seq.extend(std::iter::repeat_n(
            In::<WI, CW> {
                sample: None,
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            },
            drain,
        ));
        seq
    }

    fn run_with(seq: Vec<In<WI, CW>>) -> Vec<i128> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect()
    }

    fn run(x: &[i128], rate: usize) -> Vec<i128> {
        run_with(stimulus(x, rate, 2))
    }

    /// One reset cycle, then the first input cycle, then output.
    ///
    /// Named rather than a bare `2`, because every comparison below
    /// depends on it and a change here would otherwise look like a
    /// change in the filter.
    const LATENCY: usize = 2;

    // ---- Tier 1 / 2: it is the filter it claims to be ---------------

    #[test]
    fn default_construction() {
        let _ = Uut::default();
    }

    /// The width this configuration needs is the one it declares.
    #[test]
    fn the_test_configuration_is_at_the_bound() {
        assert_eq!(interp::gain_bits(S, RMAX, M), 3);
        assert_eq!(interp::accumulator_width(WI, S, RMAX, M), WA);
        assert_eq!(interp::rate_width(RMAX), CW);
        assert_eq!(
            interp::counter_width(RMAX),
            CW - 1,
            "carrying the rate costs a bit over counting to it"
        );
    }

    /// Matches an independently written model, sample for sample.
    #[test]
    fn matches_the_model_on_a_varying_input() {
        for rate in [2usize, 4, 8] {
            let x: Vec<i128> = (0..12).map(|k| (k % 7) - 3).collect();
            let got = run(&x, rate);
            let want = model(&x, S, rate, M);
            assert_eq!(&got[..LATENCY], &[0, 0], "rate {rate}: reset then fill");
            assert_eq!(
                &got[LATENCY..LATENCY + want.len()],
                &want[..],
                "rate {rate}"
            );
        }
    }

    /// **A constant input comes out an exact constant.**
    ///
    /// The interpolator's defining property, and it is exact rather
    /// than approximate. A constant low-rate sequence upsampled by `R`
    /// has images at every multiple of the input rate, and the
    /// `sinc^N` nulls sit at exactly those frequencies — so every image
    /// is annihilated and what remains is DC alone. Any error in the
    /// cascade's order or depth leaves a residual ripple at the input
    /// rate, which is the single most visible artefact a DUC can have.
    #[test]
    fn a_constant_input_becomes_an_exact_constant() {
        for rate in [2usize, 4, 8] {
            let got = run(&[7i128; 16], rate);
            // Settled: past the cascade's N*R*M-sample transient.
            let tail = &got[got.len() - 2 * rate - 2..got.len() - 2];
            assert!(
                tail.iter().all(|v| *v == tail[0]),
                "rate {rate}: settled output must be flat, got {tail:?}"
            );
            assert_ne!(tail[0], 0, "rate {rate}: and not flat zero");
        }
    }

    /// And it settles at exactly `(R·M)^N / R`.
    ///
    /// `(R·M)^N` would be the transfer function's DC gain; the extra
    /// `1/R` is the zero-stuffing, and getting it wrong is a factor-of-
    /// `R` amplitude error in a transmitter.
    #[test]
    fn the_settled_gain_is_the_published_ratio() {
        for rate in [2usize, 4, 8] {
            let x = 7i128;
            let got = run(&vec![x; 16], rate);
            let settled = got[got.len() - 3];
            let (num, den) = interp::dc_gain_ratio(S, rate, M);
            assert_eq!(
                settled,
                x * num as i128 / den as i128,
                "rate {rate}: expected gain {num}/{den}"
            );
        }
    }

    /// **Two stages interpolate a ramp exactly.**
    ///
    /// A second-order CIC is linear interpolation, so a low-rate ramp
    /// must come out a high-rate ramp with no staircase left in it.
    /// This is the property that distinguishes a working interpolator
    /// from one that merely holds each sample for `R` cycles, and a
    /// first-order filter would fail it.
    #[test]
    fn two_stages_interpolate_a_ramp_without_a_staircase() {
        let rate = 4usize;
        let x: Vec<i128> = (0..10).collect();
        let got = run(&x, rate);
        // Skip the fill, then look at consecutive differences: a true
        // ramp has a constant first difference.
        let settled = &got[LATENCY + 3 * rate..got.len() - 3];
        let diffs: Vec<i128> = settled.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            diffs.iter().all(|d| *d == diffs[0]),
            "second-order interpolation of a ramp must be a ramp, diffs {diffs:?}"
        );
        assert_ne!(diffs[0], 0, "and the ramp must actually rise");
    }

    /// **`input_ready` comes up once every `R` cycles, and only then.**
    ///
    /// The upstream-facing half of the contract: this widget is
    /// rate-controlling, and an upstream that ignores this signal
    /// overruns it silently.
    #[test]
    fn input_ready_fires_once_per_rate() {
        for rate in [2usize, 4, 8] {
            let uut = Uut::default();
            let seq = stimulus(&[1i128; 8], rate, 0);
            let readies: Vec<bool> = uut
                .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .map(|s| s.output.input_ready)
                .collect();
            // Drop the reset cycle, where it is forced low.
            let live = &readies[1..];
            for (n, r) in live.iter().enumerate() {
                assert_eq!(
                    *r,
                    n % rate == 0,
                    "rate {rate}: cycle {n} ready {r}, expected {}",
                    n % rate == 0
                );
            }
        }
    }

    /// **Changing the rate does not retroactively rescale the
    /// integrators.**
    ///
    /// The gotcha in [`CicInterpolate`]'s docs, pinned. On a constant
    /// input the comb section's output is zero, so nothing is being fed
    /// into the integrators and the level they are holding does not
    /// move — the output stays at the *old* rate's gain indefinitely.
    ///
    /// Measured: rate four settles at `5 · 4 = 20`, and switching to
    /// eight leaves it at 20 rather than moving to 40.
    #[test]
    fn changing_the_rate_alone_leaves_the_level_stuck() {
        let mut seq = stimulus(&[5i128; 12], 4, 0);
        seq.extend(stimulus(&[5i128; 12], 8, 0));
        let got = run_with(seq);
        let (n4, d4) = interp::dc_gain_ratio(S, 4, M);
        assert_eq!(
            got[4 * 12 - 1],
            5 * n4 as i128 / d4 as i128,
            "settled at four"
        );
        assert_eq!(
            *got.last().unwrap(),
            5 * n4 as i128 / d4 as i128,
            "still at the rate-four gain: the integrators were never re-fed"
        );
    }

    /// **A rate change plus a restart does what the caller meant.**
    ///
    /// The other half of the pair, and the usage the docs prescribe.
    /// The restart clears the integrators, so the new rate's gain is
    /// established from a clean window.
    #[test]
    fn a_restart_makes_the_new_rate_take_effect() {
        let mut seq = stimulus(&[5i128; 12], 4, 0);
        let mut second = stimulus(&[5i128; 12], 8, 0);
        second[0].restart = true;
        seq.extend(second);
        let got = run_with(seq);
        let (n8, d8) = interp::dc_gain_ratio(S, 8, M);
        assert_eq!(
            *got.last().unwrap(),
            5 * n8 as i128 / d8 as i128,
            "the restart let the new rate establish its own gain"
        );
    }

    /// **A rate of zero or one is a pass-through, not a hang.**
    ///
    /// `phase + 1 >= rate` rather than `== rate - 1` is what makes this
    /// true; the subtraction would have underflowed at zero and the
    /// equality would never have matched.
    #[test]
    fn a_degenerate_rate_takes_an_input_every_cycle() {
        for rate in [0usize, 1] {
            let uut = Uut::default();
            let seq: Vec<In<WI, CW>> = (0..8)
                .map(|_| In::<WI, CW> {
                    sample: Some(signed::<WI>(1)),
                    rate: bits::<CW>(rate as u128),
                    restart: false,
                    downstream_ready: true,
                })
                .collect();
            let readies: Vec<bool> = uut
                .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .map(|s| s.output.input_ready)
                .collect();
            assert!(
                readies[1..].iter().all(|r| *r),
                "rate {rate}: every cycle should take an input, got {readies:?}"
            );
        }
    }

    /// Lowering the rate mid-count wraps the counter immediately.
    ///
    /// Otherwise the widget would run out the remainder of the old, and
    /// longer, window before honouring the new rate — which for a rate
    /// change of 125 to 2 is a stall of over a hundred cycles that no
    /// caller expects.
    #[test]
    fn lowering_the_rate_mid_count_wraps_at_once() {
        let uut = Uut::default();
        // Start at eight, then drop to two on cycle 3, well before the
        // old window would have ended.
        let seq: Vec<In<WI, CW>> = (0..10)
            .map(|n| In::<WI, CW> {
                sample: Some(signed::<WI>(1)),
                rate: bits::<CW>(if n < 3 { 8 } else { 2 }),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        let readies: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.input_ready)
            .collect();
        let live = &readies[1..];
        // Cycle 0 takes an input; cycles 1,2 do not (rate is eight);
        // from cycle 3 the rate is two, so the counter -- sitting at 3
        // -- wraps at once and the next cycle takes an input again.
        assert!(live[0], "cycle 0 takes an input");
        assert!(!live[1] && !live[2], "cycles 1-2 wait, rate is eight");
        assert!(live[4], "cycle 4 takes one: the counter wrapped at 3");
    }

    /// **Starvation feeds zero, and reports it.**
    ///
    /// Zero and not the previous sample — see the module docs on the
    /// stuck DC offset that holding leaves behind.
    #[test]
    fn starvation_feeds_zero_and_is_reported() {
        let rate = 4usize;
        let uut = Uut::default();
        // Six inputs' worth of cycles; the third input cycle is starved.
        let seq: Vec<In<WI, CW>> = (0..6 * rate)
            .map(|n| In::<WI, CW> {
                sample: if n == 2 * rate {
                    None
                } else {
                    Some(signed::<WI>(9))
                },
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        let starved: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.starved)
            .collect();
        // Registered, so it appears one cycle after the starved input.
        let fired: Vec<usize> = starved
            .iter()
            .enumerate()
            .filter(|(_, s)| **s)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(fired, vec![1 + 2 * rate + 1], "exactly one starved cycle");
    }

    /// A starved input is the same as a zero input, exactly.
    ///
    /// The behavioural half of the claim above: not merely "reported",
    /// but reported *and* equivalent to the zero it substituted.
    #[test]
    fn a_starved_cycle_equals_an_explicit_zero() {
        let rate = 4usize;
        let build = |starve: bool| -> Vec<In<WI, CW>> {
            (0..6 * rate)
                .map(|n| In::<WI, CW> {
                    sample: if n == 2 * rate && starve {
                        None
                    } else if n / rate == 2 {
                        Some(signed::<WI>(0))
                    } else {
                        Some(signed::<WI>(9))
                    },
                    rate: bits::<CW>(rate as u128),
                    restart: false,
                    downstream_ready: true,
                })
                .collect()
        };
        assert_eq!(run_with(build(true)), run_with(build(false)));
    }

    /// Reset clears the cascade, so a second burst is not contaminated.
    #[test]
    fn reset_clears_the_cascade() {
        let rate = 4usize;
        let first = run(&[6i128; 8], rate);
        let second = run(&[6i128; 8], rate);
        assert_eq!(first, second, "reset must make the run reproducible");
        assert_eq!(first[0], 0, "and the output starts at zero");
    }

    /// **Restart re-anchors the grid and discards the old window.**
    ///
    /// A burst that follows a restart must be identical to the same
    /// burst run from reset — otherwise the previous transmission leaks
    /// into the start of this one through the integrators.
    #[test]
    fn restart_discards_the_previous_window() {
        let rate = 4usize;
        let clean = run(&[6i128; 8], rate);
        // Junk, then a restart, then the same burst.
        let mut seq: Vec<In<WI, CW>> = (0..5 * rate)
            .map(|_| In::<WI, CW> {
                sample: Some(signed::<WI>(-100)),
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        let mut burst = stimulus(&[6i128; 8], rate, 2);
        burst[0].restart = true;
        seq.extend(burst);
        let got = run_with(seq);
        // The restart lands at index 1 + 5*rate; from there the output
        // must track the clean run offset by the same latency.
        let start = 1 + 5 * rate;
        assert_eq!(
            &got[start + 1..],
            &clean[LATENCY..],
            "a restarted burst must match the same burst from reset"
        );
    }

    /// A lost output is reported rather than hidden.
    #[test]
    fn a_lost_sample_is_reported() {
        let rate = 4usize;
        let uut = Uut::default();
        let seq: Vec<In<WI, CW>> = (0..4 * rate)
            .map(|n| In::<WI, CW> {
                sample: Some(signed::<WI>(3)),
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: n != 2 * rate + 1,
            })
            .collect();
        let overruns: Vec<usize> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .enumerate()
            .filter(|(_, s)| s.output.overrun)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            overruns,
            vec![1 + 2 * rate + 1],
            "overrun is combinational on downstream_ready"
        );
    }

    /// This widget never saturates, and the field says so.
    #[test]
    fn the_exact_width_never_saturates() {
        let rate = 8usize;
        let uut = Uut::default();
        // Full-scale input, the worst case for growth.
        let seq = stimulus(&[-128i128; 16], rate, 2);
        assert!(
            uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
                .synchronous_sample()
                .all(|s| !s.output.saturated)
        );
    }

    /// **Full scale at the maximum rate stays inside the register.**
    ///
    /// The claim `W_ACC` is checked against. A DC input at negative
    /// full scale is the largest magnitude the cascade can reach, so if
    /// the growth bound is right this is the test that would catch it
    /// being wrong — the output would wrap to a positive value.
    #[test]
    fn full_scale_at_the_maximum_rate_does_not_wrap() {
        let got = run(&[-128i128; 24], RMAX);
        let settled = got[got.len() - 3];
        let (num, den) = interp::dc_gain_ratio(S, RMAX, M);
        assert_eq!(settled, -128 * num as i128 / den as i128);
        assert!(settled < 0, "a wrap would have made this positive");
    }

    /// **The numbers in `examples/cic_interpolate.rs` prose, checked.**
    ///
    /// The example's commentary quotes 80 and 160 as the settled
    /// outputs either side of the rate change. Prose drifts silently;
    /// a test does not.
    #[test]
    fn the_claims_in_the_example_prose_hold() {
        const AT_FOUR: usize = 28;
        let uut = Uut::default();
        let seq: Vec<In<WI, CW>> = (0..AT_FOUR + 40)
            .map(|n| In::<WI, CW> {
                sample: Some(signed::<WI>(20)),
                rate: bits::<CW>(if n < AT_FOUR { 4 } else { 8 }),
                restart: n == AT_FOUR,
                downstream_ready: true,
            })
            .collect();
        let got: Vec<i128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.sample.raw())
            .collect();
        assert_eq!(got[AT_FOUR], 80, "20 * (4*1)^2 / 4 = 80");
        assert_eq!(*got.last().unwrap(), 160, "20 * (8*1)^2 / 8 = 160");
        // And the restart really does clear the cascade rather than
        // ramping from 80 -- the trace shows a drop to zero.
        assert_eq!(got[AT_FOUR + 2], 0, "the restart cleared the integrators");
    }

    // ---- Tier 1: construction refuses what it cannot honour ---------

    /// **A too-narrow accumulator is refused, not tolerated.**
    ///
    /// Worse here than in a decimator: the last integrator *is* the
    /// output, so a wrap is not cancelled downstream and the DAC gets
    /// a sign flip rather than noise.
    #[test]
    #[should_panic(expected = "W_ACC must be at least")]
    fn a_narrow_accumulator_is_rejected() {
        let _ = CicInterpolate::<WI, { WA - 1 }, S, RMAX, M, CW>::default();
    }

    /// As is a counter too narrow to reach `R_MAX`.
    #[test]
    #[should_panic(expected = "CW must be wide enough")]
    fn a_narrow_counter_is_rejected() {
        let _ = CicInterpolate::<WI, WA, S, RMAX, M, { CW - 1 }>::default();
    }

    /// And a rate of one, which is not an interpolator.
    #[test]
    #[should_panic(expected = "not an interpolator")]
    fn a_unit_rate_is_rejected() {
        let _ = CicInterpolate::<WI, WA, S, 1, M, CW>::default();
    }

    // ---- Tier 3: the emitted Verilog -------------------------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [14:0] i, output wire [14:0] o);
               wire [96:0] od;
               wire [81:0] d;
               wire [81:0] q;
               assign o = od[14:0];
               top_combs c0(.clock_reset(clock_reset), .i(d[21:0]), .o(q[21:0]));
               top_comb_out c1(.clock_reset(clock_reset), .i(d[43:22]), .o(q[43:22]));
               top_integrators c2(.clock_reset(clock_reset), .i(d[65:44]), .o(q[65:44]));
               top_phase c3(.clock_reset(clock_reset), .i(d[69:66]), .o(q[69:66]));
               top_out c4(.clock_reset(clock_reset), .i(d[80:70]), .o(q[80:70]));
               top_starved c5(.clock_reset(clock_reset), .i(d[81:81]), .o(q[81:81]));
               assign d = od[96:15];
               assign od = kernel_cic_interpolate_kernel(clock_reset, i, q);
               function [96:0] kernel_cic_interpolate_kernel(input reg [1:0] arg_0, input reg [14:0] arg_1, input reg [81:0] arg_2);
                     reg [21:0] r0;
                     reg [81:0] r1;
                     // d
                     reg [81:0] r2;
                     reg [21:0] r3;
                     // d
                     reg [81:0] r4;
                     reg [3:0] r5;
                     // d
                     reg [81:0] r6;
                     reg [3:0] r7;
                     reg [0:0] r8;
                     reg [0:0] r9;
                     reg [14:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     reg [3:0] r13;
                     reg [3:0] r14;
                     reg [3:0] r15;
                     reg [3:0] r16;
                     reg [0:0] r17;
                     reg [3:0] r18;
                     reg [3:0] r19;
                     // d
                     reg [81:0] r20;
                     reg [8:0] r21;
                     reg [0:0] r22;
                     reg [7:0] r23;
                     reg [7:0] r24;
                     reg [7:0] r25;
                     reg [0:0] r26;
                     reg [10:0] r27;
                     reg [10:0] r28;
                     reg [10:0] r29;
                     reg signed [10:0] r30;
                     // starved_now
                     reg [0:0] r31;
                     // x
                     reg signed [10:0] r32;
                     // starved_now
                     reg [0:0] r33;
                     // x
                     reg signed [10:0] r34;
                     // d
                     reg [81:0] r35;
                     reg [0:0] r36;
                     reg [21:0] r37;
                     reg [21:0] r38;
                     reg [0:0] r39;
                     reg [21:0] r40;
                     reg [21:0] r41;
                     reg [10:0] r42;
                     reg signed [10:0] r43;
                     // outs
                     reg [21:0] r44;
                     reg [10:0] r45;
                     // line
                     reg [10:0] r46;
                     // cs
                     reg [21:0] r47;
                     reg signed [10:0] r48;
                     reg [10:0] r49;
                     reg signed [10:0] r50;
                     // outs
                     reg [21:0] r51;
                     reg [10:0] r52;
                     // line
                     reg [10:0] r53;
                     // cs
                     reg [21:0] r54;
                     // d
                     reg [81:0] r55;
                     // d
                     reg [81:0] r56;
                     reg signed [10:0] r57;
                     // d
                     reg [81:0] r58;
                     // feed
                     reg signed [10:0] r59;
                     reg [21:0] r60;
                     reg [0:0] r61;
                     reg [21:0] r62;
                     reg signed [10:0] r63;
                     reg signed [10:0] r64;
                     reg signed [10:0] r65;
                     // ints
                     reg [21:0] r66;
                     reg [0:0] r67;
                     reg [21:0] r68;
                     reg signed [10:0] r69;
                     reg signed [10:0] r70;
                     reg [0:0] r71;
                     reg [21:0] r72;
                     reg signed [10:0] r73;
                     reg signed [10:0] r74;
                     reg signed [10:0] r75;
                     // ints
                     reg [21:0] r76;
                     // d
                     reg [81:0] r77;
                     reg signed [10:0] r78;
                     // d
                     reg [81:0] r79;
                     reg signed [10:0] r80;
                     reg [0:0] r81;
                     reg [0:0] r82;
                     reg [0:0] r83;
                     reg [14:0] r84;
                     reg [14:0] r85;
                     reg [14:0] r86;
                     reg [14:0] r87;
                     reg [14:0] r88;
                     reg [0:0] r89;
                     reg [1:0] r90;
                     reg [0:0] r91;
                     // d
                     reg [81:0] r92;
                     // d
                     reg [81:0] r93;
                     // d
                     reg [81:0] r94;
                     // d
                     reg [81:0] r95;
                     // d
                     reg [81:0] r96;
                     // d
                     reg [81:0] r97;
                     // o
                     reg [14:0] r98;
                     // o
                     reg [14:0] r99;
                     // o
                     reg [14:0] r100;
                     // o
                     reg [14:0] r101;
                     // o
                     reg [14:0] r102;
                     // d
                     reg [81:0] r103;
                     // o
                     reg [14:0] r104;
                     reg [96:0] r105;
                     localparam l0 = 82'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 4'b0000;
                     localparam l2 = 4'b0000;
                     localparam l3 = 4'b0001;
                     localparam l4 = 4'b0001;
                     localparam l5 = 4'b0000;
                     localparam l6 = 8'b10000000;
                     localparam l7 = 11'b11100000000;
                     localparam l8 = 11'b00000000000;
                     localparam l9 = 1'b1;
                     localparam l10 = 1'b0;
                     localparam l11 = 1'b0;
                     localparam l12 = 1'b1;
                     localparam l13 = 11'sb00000000000;
                     localparam l14 = 22'b0000000000000000000000;
                     localparam l15 = 22'b0000000000000000000000;
                     localparam l16 = 11'sb00000000000;
                     localparam l17 = 11'sb00000000000;
                     localparam l18 = 11'sb00000000000;
                     localparam l19 = 11'sb00000000000;
                     localparam l20 = 15'b000000000000000;
                     localparam l21 = 1'b0;
                     localparam l22 = 22'b0000000000000000000000;
                     localparam l23 = 22'b0000000000000000000000;
                     localparam l24 = 22'b0000000000000000000000;
                     localparam l25 = 4'b0000;
                     localparam l26 = 11'sb00000000000;
                     localparam l27 = 1'b0;
                     localparam l28 = 11'sb00000000000;
                     localparam l29 = 1'b0;
                     localparam l30 = 1'b0;
                     localparam l31 = 1'b0;
                     localparam l32 = 1'b0;
                     begin
                        r90 = arg_0;
                        r10 = arg_1;
                        r1 = arg_2;
                        r0 = r1[21:0];
                        r2 = l0;
                        r2[21:0] = r0;
                        r3 = r1[43:22];
                        r4 = r2;
                        r4[43:22] = r3;
                        r5 = r1[69:66];
                        r6 = r4;
                        r6[69:66] = r5;
                        r7 = r1[69:66];
                        r8 = r7 == l1;
                        r9 = r10[13:13];
                        r11 = r8 | r9;
                        r12 = r10[13:13];
                        r13 = r1[69:66];
                        r14 = r12 ? l2 : r13;
                        r15 = r14 + l3;
                        r16 = r10[12:9];
                        r17 = r15 >= r16;
                        r18 = r14 + l4;
                        r19 = r17 ? l5 : r18;
                        r20 = r6;
                        r20[69:66] = r19;
                        r21 = r10[8:0];
                        r22 = r21[8:8];
                        r23 = r21[7:0];
                        r24 = $unsigned(r23);
                        r25 = r24 & l6;
                        r26 = |r25;
                        r27 = {{3{1'b0}}, r24};
                        r28 = r26 ? l7 : l8;
                        r29 = r27 + r28;
                        r30 = $signed(r29);
                        case (r22)
                           1'b1 : r31 = l10;
                           1'b0 : r31 = l12;
                        endcase
                        case (r22)
                           1'b1 : r32 = r30;
                           1'b0 : r32 = l13;
                        endcase
                        r33 = r11 ? r31 : l10;
                        r34 = r11 ? r32 : l13;
                        r35 = r20;
                        r35[81:81] = r33;
                        r36 = r10[13:13];
                        r37 = r1[21:0];
                        r38 = r36 ? l14 : r37;
                        r39 = r10[13:13];
                        r40 = r1[43:22];
                        r41 = r39 ? l15 : r40;
                        r42 = r38[10:0];
                        r43 = r34 - r42;
                        r44 = r41;
                        r44[10:0] = r43;
                        r45 = r38[10:0];
                        r46 = r45;
                        r46[10:0] = r34;
                        r47 = r38;
                        r47[10:0] = r46;
                        r48 = r41[10:0];
                        r49 = r38[21:11];
                        r50 = r48 - r49;
                        r51 = r44;
                        r51[21:11] = r50;
                        r52 = r38[21:11];
                        r53 = r52;
                        r53[10:0] = r48;
                        r54 = r47;
                        r54[21:11] = r53;
                        r55 = r35;
                        r55[21:0] = r54;
                        r56 = r55;
                        r56[43:22] = r51;
                        r57 = r41[21:11];
                        r58 = r11 ? r56 : r35;
                        r59 = r11 ? r57 : l16;
                        r60 = r1[65:44];
                        r61 = r10[13:13];
                        r62 = r1[65:44];
                        r63 = r62[10:0];
                        r64 = r61 ? l17 : r63;
                        r65 = r64 + r59;
                        r66 = r60;
                        r66[10:0] = r65;
                        r67 = r10[13:13];
                        r68 = r1[65:44];
                        r69 = r68[21:11];
                        r70 = r67 ? l18 : r69;
                        r71 = r10[13:13];
                        r72 = r1[65:44];
                        r73 = r72[10:0];
                        r74 = r71 ? l19 : r73;
                        r75 = r70 + r74;
                        r76 = r66;
                        r76[21:11] = r75;
                        r77 = r58;
                        r77[65:44] = r76;
                        r78 = r76[21:11];
                        r79 = r77;
                        r79[80:70] = r78;
                        r80 = r1[80:70];
                        r81 = r1[81:81];
                        r82 = r10[14:14];
                        r83 = ~r82;
                        r84 = l20;
                        r84[10:0] = r80;
                        r85 = r84;
                        r85[11:11] = r8;
                        r86 = r85;
                        r86[12:12] = r81;
                        r87 = r86;
                        r87[13:13] = r83;
                        r88 = r87;
                        r88[14:14] = l21;
                        r89 = r90[1:1];
                        r91 = |r89;
                        r92 = r79;
                        r92[21:0] = l22;
                        r93 = r92;
                        r93[43:22] = l23;
                        r94 = r93;
                        r94[65:44] = l24;
                        r95 = r94;
                        r95[69:66] = l25;
                        r96 = r95;
                        r96[80:70] = l26;
                        r97 = r96;
                        r97[81:81] = l27;
                        r98 = r88;
                        r98[10:0] = l28;
                        r99 = r98;
                        r99[11:11] = l29;
                        r100 = r99;
                        r100[12:12] = l30;
                        r101 = r100;
                        r101[13:13] = l31;
                        r102 = r101;
                        r102[14:14] = l32;
                        r103 = r91 ? r97 : r79;
                        r104 = r91 ? r102 : r88;
                        r105 = {r103, r104};
                        kernel_cic_interpolate_kernel = r105;
                     end
               endfunction
            endmodule
            module top_combs(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_comb_out(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_integrators(input wire [1:0] clock_reset, input wire [21:0] i, output reg [21:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 22'b0000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 22'b0000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_phase(input wire [1:0] clock_reset, input wire [3:0] i, output reg [3:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 4'b0000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 4'b0000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_out(input wire [1:0] clock_reset, input wire [10:0] i, output reg [10:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 11'sb00000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 11'sb00000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
            module top_starved(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 1'b0;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 1'b0;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // ---- Tier 4: iverilog agrees, both paths ----------------------

    #[test]
    fn test_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let x: Vec<i128> = (0..10).map(|k| (k % 7) - 3).collect();
        let input = stimulus(&x, 4, 2)
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    // ---- Tier 5: the waveform digest ------------------------------

    #[test]
    fn test_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let x: Vec<i128> = (0..10).map(|k| (k % 7) - 3).collect();
        let input = stimulus(&x, 4, 2)
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cic_interpolate");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["06c42c22d6e4b4a11b5c058935948ed534b05b887db525d51d5aae9865d75afd"];
        let digest = vcd.dump_to_file(root.join("interpolate.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
