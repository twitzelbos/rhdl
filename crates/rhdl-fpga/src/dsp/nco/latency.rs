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
    fn measured_latency(out: &[u128], step_at: usize) -> usize {
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
    #[test]
    fn composed_totals_are_what_the_scheduler_expects() {
        assert_eq!(PHASE_CONTROL, 2);
        assert_eq!(FREQUENCY_CONTROL, 3);
        assert_eq!(FREQUENCY_LEADS_PHASE_BY, 1);
        assert_eq!(MODULATION_CONTROL, 4);
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
