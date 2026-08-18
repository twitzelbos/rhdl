#![warn(missing_docs)]
//! `Nco` — the whole synthesizer: composers, accumulator, and
//! phase-to-amplitude wired into one widget.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+Nco+---------------+
      |                     | [18]
 freq |                  sin+----->
+---->+ frequency           | [18]
phase |                  cos+----->
+---->+ phase               | [48]
      |               master+----->
      +---------------------+
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

use super::{
    config::{self, PHASE_W},
    frequency_composer::{self, FrequencyComposer},
    phase_accumulator::{self, PhaseAccumulator},
    phase_composer::{self, PhaseComposer},
    sin_cos_linear_interp::{self, AMP_W, SinCosLinearInterp, TOTAL_W},
};

/// The complete phase-coherent NCO.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct Nco {
    /// §8.3 frequency terms → `frequency_word`.
    freq: FrequencyComposer<PHASE_W>,
    /// §8.2 phase terms → `phase_offset`.
    phase: PhaseComposer<PHASE_W>,
    /// The free-running master phase.
    acc: PhaseAccumulator<PHASE_W>,
    /// Quadrature phase-to-amplitude.
    amp: SinCosLinearInterp,
}

/// Inputs: the two term groups, unchanged from their composers.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// §8.3 frequency terms.
    pub frequency: frequency_composer::In<PHASE_W>,
    /// §8.2 phase terms.
    pub phase: phase_composer::In<PHASE_W>,
}

/// Outputs.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// Sine of the composed phase.
    pub sin: SignedBits<AMP_W>,
    /// Cosine of the composed phase.
    pub cos: SignedBits<AMP_W>,
    /// The undisturbed master trajectory, full width.
    ///
    /// Exposed so a receive mixer can share one phase reference, and so
    /// offset independence stays observable from outside.
    pub master: Bits<PHASE_W>,
}

impl SynchronousIO for Nco {
    type I = In;
    type O = Out;
    type Kernel = nco_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn nco_kernel(_cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();

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
    let truncated = (q.acc.phase >> 26).resize::<TOTAL_W>();
    d.amp = sin_cos_linear_interp::In { phase: truncated };

    let o = Out {
        sin: q.amp.sin,
        cos: q.amp.cos,
        master: q.acc.master,
    };
    (o, d)
}

/// The shift above is a literal because the kernel language wants one;
/// this keeps it honest against [`config::PHASE_TRUNCATION_BITS`].
const _: () = assert!(
    config::PHASE_TRUNCATION_BITS == 26,
    "the kernel's phase shift is written as the literal 26; if \
     PHASE_W or TOTAL_W changed, that literal must change with them"
);

#[cfg(test)]
mod tests {
    use super::super::latency;
    use super::*;
    use expect_test::expect;

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
        }
    }

    fn run(seq: Vec<In>) -> Vec<Out> {
        let uut = Nco::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = Nco::default();
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
        let sins: Vec<i128> = out.iter().skip(6).map(|o| o.sin.raw()).collect();
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
        let sins: Vec<i128> = out.iter().skip(6).map(|o| o.sin.raw()).collect();
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
    #[test]
    fn end_to_end_latency_matches_the_constants() {
        const STEP: usize = 6;
        const RESET_CYCLES: usize = 1;

        let measure = |seq: Vec<In>| -> usize {
            let out = run(seq);
            let step_out = STEP + RESET_CYCLES;
            let baseline = out[step_out - 1].sin.raw();
            out.iter()
                .enumerate()
                .skip(step_out)
                .find(|(_, o)| o.sin.raw() != baseline)
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

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Only the shape is captured: the design instantiates two 256-entry
    /// tables, so a full snapshot is ~550 lines of trigonometric
    /// constants. Module names and boundaries still catch a structural
    /// change; the behaviour is covered by Tier 4 below.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Nco::default();
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
            module top_amp_delayed"#]];
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
        let uut = Nco::default();
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
        let uut = Nco::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("nco");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["42f24a1b8fcbc7fa7ccb4867da8701fae235029d1b5baaace368f53c5c3df3b0"];
        let digest = vcd.dump_to_file(root.join("nco.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
