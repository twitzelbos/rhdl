#![warn(missing_docs)]
//! §8.4 — control latency, resolved entirely at compile time.
//!
//! > "All phase, frequency, gain, and mode changes should be described
//! > by the sample at which their effects must appear at the DAC. If a
//! > phase control takes five clocks to propagate, the local timing
//! > agent applies the register change five clocks before the desired
//! > effect sample."
//!
//! These are `usize` constants. The scheduler's arithmetic is therefore
//! evaluated by rustc and **costs nothing in the emitted RTL** — there
//! is no latency register, no configurable delay line, and nothing to
//! read back. Change a pipeline stage and the constant changes with it;
//! code that schedules against it is recompiled, not reconfigured.
//!
//! # The two control paths have different lead times
//!
//! This is the trap §8.4 names: *"Simultaneous changes to multiple
//! domains require separate latency compensation."*
//!
//! | path | composer | accumulator | phase→amp | **total** |
//! |---|---|---|---|---|
//! | phase | 1 | **0** | 1 | **[`PHASE_CONTROL`] = 2** |
//! | frequency | 1 | **1** | 1 | **[`FREQUENCY_CONTROL`] = 3** |
//!
//! The difference is in the accumulator, and it is structural rather
//! than incidental. Its kernel computes
//!
//! ```text
//! o.phase  = q.master + i.phase_offset      // combinational: 0 cycles
//! d.master = q.master + i.frequency_word    // through the register: 1 cycle
//! ```
//!
//! The offset is added to the *output*; the frequency word is added to
//! the *register*. That asymmetry is exactly what makes a phase offset
//! removable without disturbing the master trajectory — the property
//! the whole design rests on — so it is not something to normalise
//! away. It just has to be scheduled around.
//!
//! **A phase change and a frequency change that must take effect on the
//! same sample are issued one cycle apart.**
//!
//! # These constants are verified, not asserted
//!
//! A latency constant that has never been checked against the hardware
//! is a comment that the scheduler trusts with the experiment's phase
//! coherence. Every constant below has a test in this module that
//! measures the real latency in simulation and fails if they disagree.
//!
//! That now includes the **composed** totals, which is a stronger claim
//! than it sounds. [`PHASE_CONTROL`] and [`FREQUENCY_CONTROL`] are
//! measured end-to-end through [`Nco`](super::composite::Nco) in
//! `composite.rs`, because their whole path lives inside that widget.
//! [`MODULATION_CONTROL`]'s path does not — [`ModulationInput`](super::modulation::ModulationInput)
//! sits outside `Nco` — so it was checked only by restating its own
//! definition until the `harness` module below composed the chain a
//! scheduler would build and measured through it.

/// [`PhaseComposer`](super::phase_composer::PhaseComposer) — registered sum.
pub const PHASE_COMPOSER: usize = 1;

/// [`FrequencyComposer`](super::frequency_composer::FrequencyComposer) — registered sum.
pub const FREQUENCY_COMPOSER: usize = 1;

/// [`PhaseAccumulator`](super::phase_accumulator::PhaseAccumulator), from
/// `phase_offset` to `phase`.
///
/// Zero: the offset is added to the output combinationally.
pub const ACCUMULATOR_PHASE_OFFSET: usize = 0;

/// [`PhaseAccumulator`](super::phase_accumulator::PhaseAccumulator), from
/// `frequency_word` to `phase`.
///
/// One: the word is added to the master register, so it moves the
/// output from the following cycle.
pub const ACCUMULATOR_FREQUENCY_WORD: usize = 1;

/// [`SinCosLinearInterp`](super::sin_cos_linear_interp::SinCosLinearInterp)
/// — the registered block-RAM read.
///
/// One, not two. The quadrant/fine attribute DFF runs *concurrently*
/// with the table read so the two meet in the same cycle; it does not
/// add a stage. That widget's `output_matches_trigonometry` uses a
/// shift of 2, but that is a sample-stream alignment that includes the
/// prepended reset cycle -- not a hardware latency. Conflating the two
/// is how this constant was wrong on the first attempt.
pub const PHASE_TO_AMPLITUDE: usize = 1;

/// Cycles from a phase term changing to the effect appearing on
/// `(sin, cos)`.
///
/// Issue a phase change this many clocks before the sample it must
/// affect.
pub const PHASE_CONTROL: usize = PHASE_COMPOSER + ACCUMULATOR_PHASE_OFFSET + PHASE_TO_AMPLITUDE;

/// Cycles from a frequency term changing to the effect appearing on
/// `(sin, cos)`.
pub const FREQUENCY_CONTROL: usize =
    FREQUENCY_COMPOSER + ACCUMULATOR_FREQUENCY_WORD + PHASE_TO_AMPLITUDE;

/// [`ModulationInput`](super::modulation::ModulationInput) — registered
/// contribution.
pub const MODULATION_INPUT: usize = 1;

/// Cycles from a modulation *sample* to the effect appearing on
/// `(sin, cos)`.
///
/// **One more than [`FREQUENCY_CONTROL`]**, because the modulation
/// input registers before reaching the composer, where a scheduled
/// offset is applied directly. A compensation waveform must therefore
/// be issued one cycle earlier than a frequency offset that has to land
/// on the same sample — the same class of asymmetry as
/// [`FREQUENCY_LEADS_PHASE_BY`], and the reason §8.6 requires "latency
/// from modulation input to output phase effect" to be stated.
///
/// **Measured through the composed chain**, not inferred from the sum:
/// `modulation_control_latency_is_as_declared_through_the_chain` wires
/// [`ModulationInput`](super::modulation::ModulationInput) into
/// [`Nco`](super::composite::Nco) exactly as a scheduler would and
/// measures modulation-sample to `(sin, cos)`. Verified able to fail —
/// perturbing this constant by one makes it report the discrepancy.
pub const MODULATION_CONTROL: usize = MODULATION_INPUT + FREQUENCY_CONTROL;

/// How much earlier a frequency change must be issued than a phase
/// change that has to land on the same sample.
///
/// One cycle today. Named rather than left as a subtraction at the call
/// site so that a scheduler reads as "compensate for the skew" instead
/// of open-coding two magic numbers.
pub const FREQUENCY_LEADS_PHASE_BY: usize = FREQUENCY_CONTROL - PHASE_CONTROL;

const _: () = assert!(
    FREQUENCY_CONTROL >= PHASE_CONTROL,
    "FREQUENCY_LEADS_PHASE_BY would underflow; if the frequency path \
     ever becomes the shorter one, the scheduler's skew handling must \
     be revisited rather than the subtraction flipped"
);

/// Test-only composition used to measure [`MODULATION_CONTROL`] through
/// the real datapath.
///
/// **Not a shipped widget.** [`Nco`](super::composite::Nco) is
/// deliberately a subassembly: §8.4 describes a local timing agent that
/// composes and schedules these pieces, so the modulation input lives
/// outside it and a caller does this wiring. This module *is* that
/// wiring, existing so the composed latency is observable rather than
/// merely asserted. If `Nco` ever absorbs the modulation input, delete
/// this and move the measurement into `composite.rs` beside the other
/// two.
#[cfg(test)]
mod harness {
    use rhdl::prelude::*;

    use crate::dsp::nco::config::PHASE_W;
    use crate::dsp::nco::sin_cos_linear_interp::AMP_W;
    use crate::dsp::nco::{composite, frequency_composer, modulation, phase_composer};

    /// [`modulation::ModulationInput`] wired into [`composite::NcoDefault`].
    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct ModulatedNco {
        /// §8.6 stream input, whose registered `word` feeds the composer.
        modulation: modulation::ModulationInput,
        /// The oscillator, at the default phase-to-amplitude configuration.
        nco: composite::NcoDefault,
    }

    impl SynchronousIO for ModulatedNco {
        type I = modulation::In;
        type O = composite::Out<AMP_W>;
        type Kernel = modulated_nco_kernel;
    }

    /// Pure wiring, no registers of its own, so the measured latency is
    /// the chain's and not the harness's.
    #[kernel]
    #[doc(hidden)]
    pub fn modulated_nco_kernel(
        _cr: ClockReset,
        i: modulation::In,
        q: Q,
    ) -> (composite::Out<AMP_W>, D) {
        let mut d = D::dont_care();
        d.modulation = i;

        // The wiring under test: the modulation input's registered word
        // enters the frequency composer's `modulation` term, exactly as a
        // scheduler would connect it. Master frequency is zero so the only
        // thing that can move the phase is the modulation contribution.
        d.nco = composite::In {
            frequency: frequency_composer::In::<PHASE_W> {
                master: bits::<PHASE_W>(0),
                scheduled_offset: bits::<PHASE_W>(0),
                modulation: q.modulation.word,
                calibration: bits::<PHASE_W>(0),
            },
            phase: phase_composer::In::<PHASE_W> {
                pulse: bits::<PHASE_W>(0),
                frame: bits::<PHASE_W>(0),
                calibration: bits::<PHASE_W>(0),
                fine_time: bits::<PHASE_W>(0),
                trim: bits::<PHASE_W>(0),
            },
            downstream_ready: true,
        };

        (q.nco, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::nco::{
        frequency_composer::FrequencyComposer, phase_accumulator::PhaseAccumulator,
        phase_composer::PhaseComposer, sin_cos_linear_interp::SinCosLinearInterpDefault,
    };
    use rhdl::prelude::*;

    const W: usize = 32;

    /// Cycles of `with_reset` prepended to every stimulus below.
    ///
    /// Load-bearing: `synchronous_sample` yields one sample per clock
    /// **including** the reset cycles, so stimulus index `k` appears at
    /// output index `k + RESET_CYCLES`. Forgetting this reads back one
    /// cycle too many and inflates every constant in this module --
    /// which is exactly what happened on the first attempt, and is why
    /// `sin_cos_linear_interp`'s test constant of 2 is a sample-stream
    /// alignment rather than a hardware latency.
    const RESET_CYCLES: usize = 1;

    /// Hardware latency: cycles from the stimulus stepping at index
    /// `step_at` to the output responding.
    fn measured_latency<T: PartialEq + Copy>(out: &[T], step_at: usize) -> usize {
        let step_out = step_at + RESET_CYCLES;
        let baseline = out[step_out - 1];
        out.iter()
            .enumerate()
            .skip(step_out)
            .find(|(_, v)| **v != baseline)
            .map(|(i, _)| i - step_out)
            .expect("the stimulus never changed the output; the test proves nothing")
    }

    /// [`PHASE_COMPOSER`] matches the hardware.
    #[test]
    fn phase_composer_latency_is_as_declared() {
        let uut = PhaseComposer::<W>::default();
        const STEP: usize = 4;
        let seq: Vec<crate::dsp::nco::phase_composer::In<W>> = (0..12)
            .map(|k| crate::dsp::nco::phase_composer::In::<W> {
                pulse: bits::<W>(if k >= STEP { 0x1000 } else { 0 }),
                frame: bits::<W>(0),
                calibration: bits::<W>(0),
                fine_time: bits::<W>(0),
                trim: bits::<W>(0),
            })
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.raw())
            .collect();
        assert_eq!(measured_latency(&out, STEP), PHASE_COMPOSER);
    }

    /// [`FREQUENCY_COMPOSER`] matches the hardware.
    #[test]
    fn frequency_composer_latency_is_as_declared() {
        let uut = FrequencyComposer::<W>::default();
        const STEP: usize = 4;
        let seq: Vec<crate::dsp::nco::frequency_composer::In<W>> = (0..12)
            .map(|k| crate::dsp::nco::frequency_composer::In::<W> {
                master: bits::<W>(0),
                scheduled_offset: bits::<W>(if k >= STEP { 0x1000 } else { 0 }),
                modulation: bits::<W>(0),
                calibration: bits::<W>(0),
            })
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.raw())
            .collect();
        assert_eq!(measured_latency(&out, STEP), FREQUENCY_COMPOSER);
    }

    /// [`ACCUMULATOR_PHASE_OFFSET`] is zero — the offset reaches the
    /// output combinationally.
    #[test]
    fn accumulator_phase_offset_latency_is_as_declared() {
        let uut = PhaseAccumulator::<W>::default();
        const STEP: usize = 4;
        // Frequency zero, so the only thing that can move `phase` is the
        // offset.  With a non-zero frequency the accumulator advances
        // every cycle and `first_change` would measure nothing.
        let seq: Vec<crate::dsp::nco::phase_accumulator::In<W>> = (0..12)
            .map(|k| crate::dsp::nco::phase_accumulator::In::<W> {
                frequency_word: bits::<W>(0),
                phase_offset: bits::<W>(if k >= STEP { 0x1000 } else { 0 }),
            })
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.phase.raw())
            .collect();
        assert_eq!(measured_latency(&out, STEP), ACCUMULATOR_PHASE_OFFSET);
    }

    /// [`ACCUMULATOR_FREQUENCY_WORD`] is one — the word goes through
    /// the master register.
    #[test]
    fn accumulator_frequency_word_latency_is_as_declared() {
        let uut = PhaseAccumulator::<W>::default();
        const STEP: usize = 4;
        let seq: Vec<crate::dsp::nco::phase_accumulator::In<W>> = (0..12)
            .map(|k| crate::dsp::nco::phase_accumulator::In::<W> {
                frequency_word: bits::<W>(if k >= STEP { 0x1000 } else { 0 }),
                phase_offset: bits::<W>(0),
            })
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.phase.raw())
            .collect();
        assert_eq!(measured_latency(&out, STEP), ACCUMULATOR_FREQUENCY_WORD);
    }

    /// [`PHASE_TO_AMPLITUDE`] matches the hardware.
    #[test]
    fn phase_to_amplitude_latency_is_as_declared() {
        use crate::dsp::nco::sin_cos_linear_interp::{In as ScIn, TOTAL_W};
        let uut = SinCosLinearInterpDefault::default();
        const STEP: usize = 4;
        let seq: Vec<ScIn<TOTAL_W>> = (0..12)
            .map(|k| ScIn::<TOTAL_W> {
                phase: bits::<TOTAL_W>(if k >= STEP { 1 << 19 } else { 0 }),
            })
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.sin.raw() as u128)
            .collect();
        assert_eq!(measured_latency(&out, STEP), PHASE_TO_AMPLITUDE);
    }

    /// The composed totals are what the scheduler will use, so state
    /// them explicitly rather than leaving them implied by the parts.
    ///
    /// **These are the definitions, not evidence.** Each total is also
    /// measured against hardware: `PHASE_CONTROL` and
    /// `FREQUENCY_CONTROL` in `composite.rs`, `MODULATION_CONTROL` in
    /// `modulation_control_latency_is_as_declared_through_the_chain`.
    /// This test exists so that a change to a total is visible as a
    /// deliberate edit here rather than only as a distant failure.
    #[test]
    fn composed_totals_are_what_the_scheduler_expects() {
        assert_eq!(PHASE_CONTROL, 2);
        assert_eq!(FREQUENCY_CONTROL, 3);
        assert_eq!(FREQUENCY_LEADS_PHASE_BY, 1);
        assert_eq!(MODULATION_CONTROL, 4);
    }

    /// **[`MODULATION_CONTROL`] measured through a composed chain.**
    ///
    /// This was the one constant in the module checked only by arithmetic
    /// — `composed_totals_are_what_the_scheduler_expects` asserted
    /// `1 + 3 == 4`, which restates the definition and would pass even if
    /// the real latency were seven. That is exactly the "comment the
    /// scheduler trusts with the experiment's phase coherence" this
    /// module opens by warning against.
    ///
    /// It could not be measured before because the path runs
    /// [`ModulationInput`] → [`FrequencyComposer`]'s `modulation` term →
    /// accumulator → phase-to-amplitude, and `ModulationInput` sits
    /// **outside** [`Nco`]. `PHASE_CONTROL` and `FREQUENCY_CONTROL` are
    /// measured end-to-end in `composite.rs` precisely because their
    /// whole path is inside one widget.
    ///
    /// **`Nco` stays a subassembly** — decided deliberately, since §8.4
    /// describes a local timing agent that assembles and schedules these
    /// pieces. So the harness below does the wiring a scheduler would do,
    /// and measures through it. `ModulatedNco` is test-only: it exists to
    /// make the composed latency observable, not as a shipped widget.
    ///
    /// Running the two widgets separately and summing their latencies
    /// would reproduce the arithmetic rather than check it, so the point
    /// is that this composes them for real and observes `sin`.
    #[test]
    fn modulation_control_latency_is_as_declared_through_the_chain() {
        use crate::dsp::nco::modulation::{In as ModIn, MOD_W};
        use crate::rcstream::bus::Item;

        const STEP: usize = 4;
        // Large enough that ONE accumulator step moves the top TOTAL_W
        // bits: the contribution is `sample << 16`, and phase-to-amplitude
        // discards the low PHASE_TRUNCATION_BITS = 26, so a sample below
        // 2^10 would leave `sin` unchanged for many cycles and the
        // measurement would report the truncation delay instead of the
        // control latency.
        const SAMPLE: i128 = 32_000;

        let uut = super::harness::ModulatedNco::default();
        let seq: Vec<ModIn> = (0..16)
            .map(|k| ModIn {
                stream: Some(Item::<SignedBits<MOD_W>, ()> {
                    data: signed::<MOD_W>(if k >= STEP { SAMPLE } else { 0 }),
                    frame: (),
                }),
            })
            .collect();
        let out: Vec<i128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| match s.output.stream.data {
                Some(item) => item.data.im.raw(),
                None => panic!("the NCO is isochronous and must emit every cycle"),
            })
            .collect();

        assert_eq!(
            measured_latency(&out, STEP),
            MODULATION_CONTROL,
            "modulation sample to (sin, cos) took a different number of \
             cycles than MODULATION_CONTROL declares.  A scheduler issuing \
             a compensation waveform this many clocks early would land it \
             on the wrong sample."
        );
    }

    /// [`MODULATION_INPUT`] matches the hardware.
    #[test]
    fn modulation_input_latency_is_as_declared() {
        use crate::dsp::nco::modulation::{In as ModIn, MOD_W, ModulationInput};
        use crate::rcstream::bus::Item;
        let uut = ModulationInput::default();
        const STEP: usize = 4;
        let seq: Vec<ModIn> = (0..12)
            .map(|k| ModIn {
                stream: Some(Item::<SignedBits<MOD_W>, ()> {
                    data: signed::<MOD_W>(if k >= STEP { 1000 } else { 0 }),
                    frame: (),
                }),
            })
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.word.raw())
            .collect();
        assert_eq!(measured_latency(&out, STEP), MODULATION_INPUT);
    }
}
