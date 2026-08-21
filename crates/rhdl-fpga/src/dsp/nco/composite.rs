#![warn(missing_docs)]
//! `Nco` — the whole synthesizer: composers, accumulator, and
//! phase-to-amplitude wired into one widget.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+Nco+-----------------------+
      |                             |
+---->+ frequency                   |
      |          stream: RCStream   |
+---->+ phase    <Iq<18>, SyncMark> +----->
      |                             |
+---->+ downstream_ready            |
      |                      master +----->
      |                     overrun +----->
      +-----------------------------+
")]
//!
//! # The truncation this widget exists to make explicit
//!
//! The accumulator carries [`config::PHASE_W`] = 48 bits; phase-to-
//! amplitude consumes [`config::TOTAL_W`] = 22. **The top 22 bits are
//! taken and the low [`config::PHASE_TRUNCATION_BITS`] = 26 discarded.**
//!
//! That is not a detail. It *is* the phase truncation the entire spur
//! analysis in [`super::model`] is about — the reason worst-case SFDR
//! tracks `6.02·P − 3.92`, and the reason tuning words with short
//! remainder periods concentrate error into few strong spurs.
//!
//! Taking the *low* 22 bits instead would produce a signal that still
//! looks like a waveform on a scope and is completely wrong: the output
//! would advance through a full turn every 2²² samples regardless of
//! the commanded frequency. `truncation_takes_the_high_bits` fails if
//! the shift is dropped, and the accompanying test shows the low-bit
//! version producing an unrelated frequency.
//!
//! # The oscillator marks its own samples
//!
//! The output stream is framed with [`SyncMark`], and `sync` is
//! asserted on **the first sample a configuration change affects**.
//!
//! A configuration change is detected here, against last cycle's terms,
//! and the resulting pulse is delayed by that path's own control
//! latency so it re-emerges on exactly the sample it describes. The
//! frequency and phase paths get separate delay lines because their
//! latencies differ by [`FREQUENCY_LEADS_PHASE_BY`](super::latency::FREQUENCY_LEADS_PHASE_BY);
//! a change to each, issued that many cycles apart so both land
//! together, produces one marker rather than two.
//!
//! **Why here and not downstream.** Tagging at a downstream gate needs
//! that gate to know this oscillator's control latency and to be told
//! when a change was issued. Both are already known here, and a second
//! copy of a latency constant is a copy that can drift. Marking at the
//! source makes the marker and the sample it names come out of the same
//! widget, so they cannot disagree about which sample that is. It also
//! reverses an earlier decision — see [`Out::stream`].
//!
//! **Why it is not self-fulfilling.** The delay depths come *from*
//! `latency::FREQUENCY_CONTROL` and `latency::PHASE_CONTROL`, so the
//! marker alone cannot vouch for those constants. What tests them is
//! `the_frequency_marker_lands_on_the_first_affected_sample`, which
//! finds the first sample whose *value* departs from the trajectory it
//! was on — a fact about the datapath, not about any constant — and
//! requires the marker to be on it.
//!
//! # Why the output is an `RCStream`, and what that does not mean
//!
//! The stream type buys **relay insertion**: per Carloni's theorem a
//! relay station may be placed anywhere on the connection, adding one
//! cycle of latency without changing throughput or functional
//! behaviour. On a 125 MHz chain crossing a Zynq that is the mechanism
//! for timing closure, and the added cycle folds straight into
//! [`super::latency`] — the scheduler's lead time becomes
//! `PHASE_CONTROL + relays_on_path`.
//!
//! It does **not** mean the NCO tolerates backpressure. Latency
//! *insensitivity* is about being correct under any fixed pipeline
//! depth, not about surviving data-dependent stalls. This oscillator
//! cannot stall: its phase represents absolute elapsed time, so pausing
//! the accumulator does not delay the waveform, it desynchronises it
//! from the timebase. `downstream_ready` going low therefore means a
//! sample was lost, and [`Out::overrun`] says so.
//!
//! Relays are compatible with that — a relay downstream of an
//! always-ready sink never deasserts. An elastic buffer with
//! data-dependent occupancy is not, and belongs downstream of the
//! acquisition gate where data becomes timestamped and latency stops
//! mattering.
//!
//! # Latency
//!
//! Wiring adds no registers, so the totals are exactly the constants in
//! [`super::latency`]: [`PHASE_CONTROL`](super::latency::PHASE_CONTROL) = 2 cycles,
//! [`FREQUENCY_CONTROL`](super::latency::FREQUENCY_CONTROL) = 3. `end_to_end_latency_matches_the_constants`
//! measures both through this widget, which is the first place they can
//! be checked as a *chain* rather than stage by stage.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/nco.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/nco.md")]

use rhdl::prelude::*;

use crate::core::delay::Delay;
use crate::core::dff;
use crate::dsp::iq::Iq;
use crate::dsp::sync::{SyncMark, clear, when};
use crate::rcstream::bus::{Item, RCStream};

use super::{
    config::{self, PHASE_W},
    frequency_composer::{self, FrequencyComposer},
    latency,
    phase_accumulator::{self, PhaseAccumulator},
    phase_composer::{self, PhaseComposer},
    sin_cos_linear_interp::{self, SinCosLinearInterp},
};

/// The complete phase-coherent NCO.
///
/// Generic over the phase-to-amplitude configuration, which is what makes
/// a wider oscillator reachable from the top level rather than only
/// inside the phase-to-amplitude widget. See
/// [`SinCosLinearInterp`] for what the five widths mean and
/// [`NcoDefault`] for the validated default.
///
/// `TRUNC` is the number of accumulator bits discarded at the
/// phase-to-amplitude boundary and must equal `PHASE_W - TOTAL_W`; a
/// runtime assertion in `Default` enforces it, because the kernel needs
/// the shift as a value.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Nco<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
    const TRUNC: usize,
> where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    /// §8.3 frequency terms → `frequency_word`.
    freq: FrequencyComposer<PHASE_W>,
    /// §8.2 phase terms → `phase_offset`.
    phase: PhaseComposer<PHASE_W>,
    /// The free-running master phase.
    acc: PhaseAccumulator<PHASE_W>,
    /// Quadrature phase-to-amplitude.
    amp: SinCosLinearInterp<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W>,
    /// Last cycle's frequency terms, for change detection.
    prev_freq: dff::DFF<frequency_composer::In<PHASE_W>>,
    /// Last cycle's phase terms, for change detection.
    prev_phase: dff::DFF<phase_composer::In<PHASE_W>>,
    /// A frequency-term change, delayed to the sample it first affects.
    ///
    /// Depth is [`latency::FREQUENCY_CONTROL`] — see the module docs on
    /// why the two paths cannot share one delay line.
    freq_tag: Delay<bool, { latency::FREQUENCY_CONTROL }>,
    /// A phase-term change, delayed to the sample it first affects.
    ///
    /// Depth is [`latency::PHASE_CONTROL`], one shorter than the
    /// frequency line.
    phase_tag: Delay<bool, { latency::PHASE_CONTROL }>,
}

/// The validated default NCO: phase-to-amplitude at 8/12/22/18/48.
///
/// `AMP_W = 18` is the DSP48's native multiplier port width, so this is
/// the configuration whose fine rotation fits one slice per multiply.
/// Wider configurations trade slices and block RAM for effective bits —
/// see [`SinCosLinearInterp`]'s validated-configuration table.
pub type NcoDefault = Nco<
    { sin_cos_linear_interp::TBL_W },
    { sin_cos_linear_interp::FINE_W },
    { sin_cos_linear_interp::TOTAL_W },
    { sin_cos_linear_interp::AMP_W },
    { sin_cos_linear_interp::INT_W },
    { config::PHASE_TRUNCATION_BITS },
>;

impl<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
    const TRUNC: usize,
> Default for Nco<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W, TRUNC>
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    fn default() -> Self {
        assert_eq!(
            TRUNC,
            PHASE_W - TOTAL_W,
            "TRUNC is the phase truncation and must be PHASE_W - TOTAL_W; \
             the kernel uses it as the shift amount, and a wrong value \
             takes the wrong end of the accumulator"
        );
        Self {
            freq: FrequencyComposer::default(),
            phase: PhaseComposer::default(),
            acc: PhaseAccumulator::default(),
            amp: SinCosLinearInterp::default(),
            prev_freq: dff::DFF::default(),
            prev_phase: dff::DFF::default(),
            freq_tag: Delay::default(),
            phase_tag: Delay::default(),
        }
    }
}

/// Inputs: the two term groups, plus the downstream ready.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// §8.3 frequency terms.
    pub frequency: frequency_composer::In<PHASE_W>,
    /// §8.2 phase terms.
    pub phase: phase_composer::In<PHASE_W>,
    /// Downstream's ready, per the `RCStream` contract.
    ///
    /// **The NCO does not stall when this is low, and cannot.** Its
    /// phase represents absolute elapsed time, so pausing the
    /// accumulator would not delay the waveform, it would desynchronise
    /// it from the timebase. A low `ready` therefore means a sample was
    /// lost, which [`Out::overrun`] reports rather than hides.
    pub downstream_ready: bool,
}

/// Outputs.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const AMP_W: usize>
where
    rhdl::bits::W<AMP_W>: BitWidth,
{
    /// The complex sample stream, framed with [`SyncMark`].
    ///
    /// **`sync` is asserted on the first sample affected by a
    /// configuration change** — see the module docs. One bit, so
    /// `Item<Iq<18>, SyncMark>` is 37 rather than 36.
    ///
    /// This reverses an earlier decision that read:
    ///
    /// > `F = ()`: nothing is framed in the timed domain — `sync` is
    /// > inserted downstream at the acquisition gate.
    ///
    /// Tagging downstream requires the tagger to know this
    /// oscillator's control latency and to be told when a change was
    /// issued. Both are things the oscillator already knows, and a
    /// second copy of a latency constant is a copy that can drift.
    /// Tagging at the source makes the marker and the sample it refers
    /// to come from the same widget, so they cannot disagree about
    /// which sample that is.
    ///
    /// `stream.ready` is vacuously `true` — the NCO has no upstream to
    /// backpressure. It is present because the type carries both
    /// directions, not because it means anything here.
    pub stream: RCStream<Iq<AMP_W>, SyncMark>,
    /// The undisturbed master trajectory, full width.
    ///
    /// Not part of the stream: it is a shared phase *reference*, not a
    /// sample. A receive mixer consumes it to stay coherent with this
    /// oscillator, and offset independence stays observable from
    /// outside.
    pub master: Bits<PHASE_W>,
    /// A sample was presented while `downstream_ready` was low, and is
    /// gone.
    ///
    /// This is a design error being reported, not a condition to
    /// handle: the timed domain must hold `ready` true. Relay stations
    /// are fine — a Carloni relay downstream of an always-ready sink
    /// never deasserts — but an elastic buffer with data-dependent
    /// occupancy does not belong on this path. Surfaced because a
    /// silently dropped sample is exactly the failure this codebase has
    /// shipped before.
    pub overrun: bool,
}

impl<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
    const TRUNC: usize,
> SynchronousIO for Nco<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W, TRUNC>
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    type I = In;
    type O = Out<AMP_W>;
    type Kernel = nco_kernel<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W, TRUNC>;
}

#[kernel]
#[doc(hidden)]
pub fn nco_kernel<
    const TBL_W: usize,
    const FINE_W: usize,
    const TOTAL_W: usize,
    const AMP_W: usize,
    const INT_W: usize,
    const TRUNC: usize,
>(
    cr: ClockReset,
    i: In,
    q: Q<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W, TRUNC>,
) -> (Out<AMP_W>, D<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W, TRUNC>)
where
    rhdl::bits::W<TBL_W>: BitWidth,
    rhdl::bits::W<FINE_W>: BitWidth,
    rhdl::bits::W<TOTAL_W>: BitWidth,
    rhdl::bits::W<AMP_W>: BitWidth,
    rhdl::bits::W<INT_W>: BitWidth,
{
    let mut d = D::<TBL_W, FINE_W, TOTAL_W, AMP_W, INT_W, TRUNC>::dont_care();

    d.freq = i.frequency;
    d.phase = i.phase;

    // The composers' registered outputs drive the accumulator in the
    // same cycle, so the wiring adds no latency of its own.
    d.acc = phase_accumulator::In::<PHASE_W> {
        frequency_word: q.freq,
        phase_offset: q.phase,
    };

    // *** The phase truncation. Top TOTAL_W bits, low 26 discarded. ***
    // See the module docs: taking the low bits instead yields a
    // plausible-looking waveform at an unrelated frequency.
    let truncated = (q.acc.phase >> bits::<8>(TRUNC as u128)).resize::<TOTAL_W>();
    d.amp = sin_cos_linear_interp::In::<TOTAL_W> { phase: truncated };

    // re is in-phase (cosine), im is quadrature (sine) -- see
    // `crate::dsp::iq::Iq`.
    let sample = Iq::<AMP_W> {
        re: q.amp.cos,
        im: q.amp.sin,
    };

    // *** Self-tagging. ***
    //
    // A configuration change is detected combinationally against last
    // cycle's terms, then delayed by that path's own control latency so
    // the pulse re-emerges on precisely the sample it first affects.
    //
    // The two lines have different depths on purpose. A frequency term
    // reaches the output through the accumulator's register and a phase
    // term does not (`latency::FREQUENCY_LEADS_PHASE_BY`), so a
    // simultaneous change issued the required cycle apart lands both
    // pulses on the same output sample, where the OR collapses them
    // into one marker.
    d.prev_freq = i.frequency;
    d.prev_phase = i.phase;
    d.freq_tag = i.frequency != q.prev_freq;
    d.phase_tag = i.phase != q.prev_phase;
    let tagged = q.freq_tag || q.phase_tag;

    let mut o = Out::<AMP_W> {
        stream: RCStream::<Iq<AMP_W>, SyncMark> {
            data: Some(Item::<Iq<AMP_W>, SyncMark> {
                data: sample,
                frame: when(tagged),
            }),
            ready: true,
        },
        master: q.acc.master,
        overrun: !i.downstream_ready,
    };

    if cr.reset.any() {
        o.overrun = false;
        // A marker emitted during reset would anchor a timing
        // relationship to a sample the datapath has not produced.
        o.stream.data = Some(Item::<Iq<AMP_W>, SyncMark> {
            data: sample,
            frame: clear(),
        });
    }
    (o, d)
}

/// The default configuration's truncation, kept as a build-time check on
/// [`config::PHASE_TRUNCATION_BITS`] itself.
///
/// The kernel no longer contains a literal shift — it uses the `TRUNC`
/// generic, which `Default` checks equals `PHASE_W - TOTAL_W` for *every*
/// instantiation rather than only for this one. This assertion remains
/// because `config` states the figure independently and the two should
/// not drift.
const _: () = assert!(
    config::PHASE_TRUNCATION_BITS == PHASE_W - sin_cos_linear_interp::TOTAL_W,
    "config::PHASE_TRUNCATION_BITS disagrees with PHASE_W - TOTAL_W"
);

#[cfg(test)]
mod tests {
    use super::super::latency;
    use super::*;
    use expect_test::expect;
    use sin_cos_linear_interp::AMP_W;

    /// The default configuration, so these tests read as before the
    /// widths became generic and the committed snapshots stay valid.
    type Uut = NcoDefault;
    /// [`Out`] at the default amplitude width.
    type UutOut = Out<AMP_W>;

    fn input(freq_word: u128, phase_off: u128) -> In {
        In {
            frequency: frequency_composer::In::<PHASE_W> {
                master: bits::<PHASE_W>(freq_word),
                scheduled_offset: bits::<PHASE_W>(0),
                modulation: bits::<PHASE_W>(0),
                calibration: bits::<PHASE_W>(0),
            },
            phase: phase_composer::In::<PHASE_W> {
                pulse: bits::<PHASE_W>(phase_off),
                frame: bits::<PHASE_W>(0),
                calibration: bits::<PHASE_W>(0),
                fine_time: bits::<PHASE_W>(0),
                trim: bits::<PHASE_W>(0),
            },
            downstream_ready: true,
        }
    }

    /// The quadrature component (sine) of a sample, for tests that
    /// reason about the waveform.
    /// Is this sample carrying the marker?
    fn sync_of(o: &UutOut) -> bool {
        match o.stream.data {
            Some(item) => item.frame.sync,
            None => panic!("the NCO must emit on every cycle -- it is isochronous"),
        }
    }

    /// Index of the single marked sample, or `None`.  Panics if more
    /// than one is marked, because every caller below expects exactly
    /// one and a second marker would otherwise pass unnoticed.
    fn sole_marked(out: &[UutOut]) -> Option<usize> {
        let marks: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| sync_of(o))
            .map(|(i, _)| i)
            .collect();
        assert!(
            marks.len() <= 1,
            "expected at most one marker, found {} at {:?}",
            marks.len(),
            marks
        );
        marks.first().copied()
    }

    /// Index of the first sample whose value departs from the trajectory
    /// it had been on.
    ///
    /// **Determined by the datapath, not by any latency constant.** That
    /// is the whole point: comparing this against the marker's index
    /// tests the constant instead of restating it.
    fn first_deviation(out: &[UutOut], after: usize) -> usize {
        let baseline = im_of(&out[after - 1]);
        out.iter()
            .enumerate()
            .skip(after)
            .find(|(_, o)| im_of(o) != baseline)
            .map(|(i, _)| i)
            .expect("the stimulus never moved the output; the test proves nothing")
    }

    fn im_of(o: &UutOut) -> i128 {
        match o.stream.data {
            Some(item) => item.data.im.raw(),
            None => panic!("the NCO must emit on every cycle -- it is isochronous"),
        }
    }

    fn run(seq: Vec<In>) -> Vec<UutOut> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = Uut::default();
    }

    /// The commanded frequency is the frequency that comes out.
    ///
    /// This is the test that pins the truncation **direction**. A word
    /// of `2^PHASE_W / 64` must produce exactly one output cycle every
    /// 64 samples. Taking the *low* 22 bits instead of the high ones
    /// gives a signal that still oscillates -- so a "does it wiggle"
    /// check would pass -- at a completely unrelated rate.
    ///
    /// Verified able to fail: dropping the `>> 26` in the kernel makes
    /// the observed period collapse and the assertion reports it.
    #[test]
    fn truncation_takes_the_high_bits() {
        const PERIOD: usize = 64;
        let word = (1u128 << PHASE_W) / PERIOD as u128;
        const CYCLES: usize = 10;
        let out = run(vec![input(word, 0); PERIOD * CYCLES + 8]);

        // Count rising zero crossings of sin, skipping the pipeline fill.
        let sins: Vec<i128> = out.iter().skip(6).map(im_of).collect();
        let crossings = sins.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        let expected = CYCLES;
        assert!(
            crossings.abs_diff(expected) <= 1,
            "commanded one cycle per {PERIOD} samples over {CYCLES} cycles, \
             but saw {crossings} rising zero crossings. If this is far off, \
             the phase truncation is taking the wrong end of the accumulator."
        );
    }

    /// A commanded frequency in Hz, through [`config::tuning_word`],
    /// produces that frequency at the output.
    ///
    /// This is what ties the units layer to the hardware: without it
    /// `tuning_word` is arithmetic nobody has checked against a wave.
    #[test]
    fn a_commanded_frequency_in_hz_comes_out() {
        // 125 MHz / 64 = 1.953125 MHz, chosen so the period is an exact
        // sample count and the test does not depend on rounding.
        let hz = config::F_SAMPLE_HZ as u128 / 64;
        let word = config::tuning_word(hz * 1_000_000);
        const CYCLES: usize = 8;
        let out = run(vec![input(word, 0); 64 * CYCLES + 8]);
        let sins: Vec<i128> = out.iter().skip(6).map(im_of).collect();
        let crossings = sins.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        assert!(
            crossings.abs_diff(CYCLES) <= 1,
            "commanded {hz} Hz (word {word}); expected ~{CYCLES} cycles, saw {crossings}"
        );
    }

    /// The composed latencies hold as a **chain**, not just stage by
    /// stage.
    ///
    /// [`super::latency`] measures each stage in isolation; this is the
    /// first place the sum can be checked against the real assembly,
    /// which is the only version the scheduler actually cares about.
    /// **The marker lands on the first sample the change affects.**
    ///
    /// This is the test that makes the marker worth anything. The
    /// marker's position comes from a delay line whose depth is
    /// `latency::FREQUENCY_CONTROL`; the deviation's position comes
    /// from the real datapath. Asserting they are equal therefore tests
    /// the constant rather than restating it — which is exactly the
    /// distinction `latency.rs` opens by drawing, and the one that
    /// `MODULATION_CONTROL` failed for a while by asserting `1 + 3 == 4`.
    ///
    /// Verified able to fail: with `freq_tag`'s depth set to
    /// `FREQUENCY_CONTROL + 1` this test and
    /// `a_coscheduled_pair_produces_one_marker` both fail, while the
    /// phase-path test — whose line was untouched — still passes. The
    /// mirror perturbation on `phase_tag` fails the mirror pair.
    #[test]
    fn the_frequency_marker_lands_on_the_first_affected_sample() {
        const STEP: usize = 6;
        const RESET_CYCLES: usize = 1;
        let seq: Vec<In> = (0..20)
            .map(|k| input(if k >= STEP { 1 << (PHASE_W - 3) } else { 0 }, 0))
            .collect();
        let out = run(seq);

        let deviation = first_deviation(&out, STEP + RESET_CYCLES);
        let marker = sole_marked(&out).expect("the change was never marked");
        assert_eq!(
            marker,
            deviation,
            "the marker is on sample {marker} but the waveform first \
             changes on sample {deviation}; a consumer aligning on the \
             marker would be off by {} cycles",
            marker as i64 - deviation as i64
        );
    }

    /// The same claim for the phase path, whose latency is one shorter.
    ///
    /// Worth testing separately rather than trusting the frequency case:
    /// the two paths have different depths precisely because the
    /// accumulator adds the offset to its *output* and the frequency
    /// word to its *register*, so a single shared delay line would be
    /// right for one path and wrong for the other.
    #[test]
    fn the_phase_marker_lands_on_the_first_affected_sample() {
        const STEP: usize = 6;
        const RESET_CYCLES: usize = 1;
        let seq: Vec<In> = (0..20)
            .map(|k| input(0, if k >= STEP { 1 << (PHASE_W - 3) } else { 0 }))
            .collect();
        let out = run(seq);

        let deviation = first_deviation(&out, STEP + RESET_CYCLES);
        let marker = sole_marked(&out).expect("the change was never marked");
        assert_eq!(marker, deviation);
    }

    /// **A phase and a frequency change scheduled onto the same sample
    /// produce exactly one marker.**
    ///
    /// This is `latency::FREQUENCY_LEADS_PHASE_BY` observed rather than
    /// asserted. The scheduler issues the frequency change one cycle
    /// before the phase change so that both land together; if the two
    /// delay lines were the same depth, the pulses would emerge on
    /// different samples and `sole_marked` would find two.
    #[test]
    fn a_coscheduled_pair_produces_one_marker() {
        const FREQ_AT: usize = 6;
        const PHASE_AT: usize = FREQ_AT + latency::FREQUENCY_LEADS_PHASE_BY;
        const RESET_CYCLES: usize = 1;

        let seq: Vec<In> = (0..20)
            .map(|k| {
                input(
                    if k >= FREQ_AT { 1 << (PHASE_W - 3) } else { 0 },
                    if k >= PHASE_AT { 1 << (PHASE_W - 4) } else { 0 },
                )
            })
            .collect();
        let out = run(seq);

        let marker = sole_marked(&out).expect("the coscheduled change was never marked");
        assert_eq!(
            marker,
            FREQ_AT + RESET_CYCLES + latency::FREQUENCY_CONTROL,
            "the two pulses did not coincide on the intended sample"
        );
        assert_eq!(
            marker,
            PHASE_AT + RESET_CYCLES + latency::PHASE_CONTROL,
            "the phase path did not agree with the frequency path"
        );
    }

    /// A configuration that never changes is never marked.
    ///
    /// The marker means "a change first affects this sample", so a
    /// steadily-running oscillator must emit none — otherwise every
    /// consumer sees spurious anchors and the alignment contract is
    /// worthless.
    #[test]
    fn a_steady_configuration_is_never_marked() {
        // Non-zero and constant from the first stimulus cycle.  The
        // registered previous-terms start at their `Default`, which is
        // zero, so a non-zero constant still steps once on entry --
        // that step is a real change and is expected to be marked.
        // What must not happen is a *second* marker later.
        let seq: Vec<In> = (0..20).map(|_| input(1 << (PHASE_W - 3), 0)).collect();
        let out = run(seq);
        let marks: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| sync_of(o))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            marks.len(),
            1,
            "a constant configuration produced {} markers at {:?}; only \
             the initial application of it is a change",
            marks.len(),
            marks
        );
    }

    /// Nothing is marked while reset is asserted.
    ///
    /// A marker during reset would anchor a timing relationship to a
    /// sample the datapath has not produced yet.
    #[test]
    fn reset_never_marks() {
        const RESET_CYCLES: usize = 4;
        let uut = Uut::default();
        let seq: Vec<In> = (0..12).map(|_| input(1 << (PHASE_W - 3), 0)).collect();
        let out: Vec<UutOut> = uut
            .run(seq.into_iter().with_reset(RESET_CYCLES).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect();
        for (k, o) in out.iter().take(RESET_CYCLES).enumerate() {
            assert!(!sync_of(o), "sample {k} was marked during reset");
        }
    }

    #[test]
    fn end_to_end_latency_matches_the_constants() {
        const STEP: usize = 6;
        const RESET_CYCLES: usize = 1;

        let measure = |seq: Vec<In>| -> usize {
            let out = run(seq);
            let step_out = STEP + RESET_CYCLES;
            let baseline = im_of(&out[step_out - 1]);
            out.iter()
                .enumerate()
                .skip(step_out)
                .find(|(_, o)| im_of(o) != baseline)
                .map(|(i, _)| i - step_out)
                .expect("the stimulus never moved the output")
        };

        // Master frequency zero, so only the stepped term can move sin.
        let phase_seq: Vec<In> = (0..20)
            .map(|k| input(0, if k >= STEP { 1 << (PHASE_W - 3) } else { 0 }))
            .collect();
        assert_eq!(
            measure(phase_seq),
            latency::PHASE_CONTROL,
            "phase path latency disagrees with latency::PHASE_CONTROL"
        );

        let freq_seq: Vec<In> = (0..20)
            .map(|k| input(if k >= STEP { 1 << (PHASE_W - 3) } else { 0 }, 0))
            .collect();
        assert_eq!(
            measure(freq_seq),
            latency::FREQUENCY_CONTROL,
            "frequency path latency disagrees with latency::FREQUENCY_CONTROL"
        );
    }

    /// A sample presented while downstream is not ready is reported,
    /// not hidden.
    ///
    /// The NCO cannot stall -- its phase is absolute time -- so a low
    /// `ready` means the sample is gone. This codebase has shipped a
    /// silently dropped item before (`rcstream::credit::CreditSink`),
    /// which is why the loss is surfaced rather than assumed away.
    #[test]
    fn a_lost_sample_is_reported() {
        let word = (1u128 << PHASE_W) / 64;
        let mut seq: Vec<In> = (0..16).map(|_| input(word, 0)).collect();
        for s in seq.iter_mut().take(12).skip(8) {
            s.downstream_ready = false;
        }
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.overrun),
            "downstream went not-ready for four cycles and nothing reported a loss"
        );
        assert!(
            out.iter().any(|o| !o.overrun),
            "overrun is stuck high, so it reports nothing"
        );
        // And the oscillator keeps running regardless: phase is time.
        let a = im_of(&out[6]);
        let b = im_of(&out[14]);
        assert_ne!(
            a, b,
            "the accumulator must not stall when downstream stalls"
        );
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Only the shape is captured: the design instantiates two 256-entry
    /// tables, so a full snapshot is ~550 lines of trigonometric
    /// constants. Module names and boundaries still catch a structural
    /// change; the behaviour is covered by Tier 4 below.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_freq
            module top_freq_sum
            module top_phase
            module top_phase_sum
            module top_acc
            module top_acc_master
            module top_amp
            module top_amp_sin_tbl
            module top_amp_cos_tbl
            module top_amp_delayed
            module top_prev_freq
            module top_prev_phase
            module top_freq_tag
            module top_freq_tag_dffs
            module top_freq_tag_dffs_c0
            module top_freq_tag_dffs_c1
            module top_freq_tag_dffs_c2
            module top_phase_tag
            module top_phase_tag_dffs
            module top_phase_tag_dffs_c0
            module top_phase_tag_dffs_c1"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    fn hdl_stimulus() -> Vec<In> {
        let word = (1u128 << PHASE_W) / 64;
        (0..48u128)
            .map(|k| {
                input(
                    word,
                    if (16..32).contains(&k) {
                        1 << (PHASE_W - 2)
                    } else {
                        0
                    },
                )
            })
            .collect()
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_nco_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let opts = TestBenchOptions::default().skip(4);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_nco_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("nco");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["9992207af457a7f3890b7c8b500f5050d8cf107f7a080c4c729ddb9b7c55586c"];
        let digest = vcd.dump_to_file(root.join("nco.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
