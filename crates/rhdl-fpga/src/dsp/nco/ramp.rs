#![warn(missing_docs)]
//! `FrequencyRamp` — §8.5 scheduled frequency segments (ramps and
//! chirps).
//!
//! > "Ramps can be represented as scheduled segments with start time,
//! > end time, start word, end word, and interpolation rule. Hardware
//! > updates the frequency word deterministically each sample."
//!
//! A linear chirp *is* a linear frequency ramp, so one widget covers
//! both. The segment is loaded by the timing agent; the hardware then
//! advances the word every sample with no further control traffic.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+FrequencyRamp+------+
      |                      |
+---->+ load                 | [48]
      |                 word +----->
+---->+ start_word           |
      |              running +----->
+---->+ end_word             |
      |                 done +----->
+---->+ step                 |
      |                      |
+---->+ samples              |
      +----------------------+
")]
//!
//! # Why the accumulator carries fractional bits
//!
//! This is the whole design, and an integer accumulator gets it
//! silently wrong. The per-sample step for a ramp of `Δf` over `N`
//! samples is `Δword / N`, and at 125 MHz with a 48-bit word:
//!
//! | ramp | step per sample | as an integer |
//! |---|---|---|
//! | 1 MHz in 1 ms | 18 014 398.5 LSB | 18 014 398 |
//! | 10 kHz in 10 ms | 18 014.4 LSB | 18 014 |
//! | **1 Hz in 1 s** | **0.018 LSB** | **0** |
//! | **0.1 Hz in 1 s** | **0.0018 LSB** | **0** |
//!
//! A slow ramp's step rounds to **zero**, so an integer accumulator
//! emits a flat line and reports success. That is precisely the regime
//! adiabatic sweeps, shimming and field-drift compensation live in —
//! the failure would appear as an experiment that quietly did not
//! sweep.
//!
//! So the accumulator is [`ACC_W`] = 64 bits: the 48-bit word plus
//! [`FRAC_W`] = 16 fractional bits, with the output taking the top 48.
//! At 0.0018 LSB/sample the step is still ~118 quanta, so the slowest
//! interesting ramp is represented to under 1%.
//!
//! `a_ramp_slower_than_one_lsb_per_sample_still_moves` is the test that
//! pins this; it fails against an integer accumulator.
//!
//! # Why the endpoint is snapped
//!
//! On the final sample the accumulator is *loaded* with `end_word`
//! rather than stepped to it. Any rounding in `step` would otherwise
//! accumulate over `N` samples and leave the segment ending at an
//! almost-right frequency — and a chirp that ends 3 Hz off is a
//! chirp whose next phase-coherent segment starts wrong.
//!
//! Snapping makes the endpoint **exact by construction**, which is the
//! "numerical behavior remains defined" §8.5 asks for. The
//! discontinuity it introduces is bounded by the accumulated rounding
//! error, which the fractional bits already keep under 1 LSB.
//!
//! # Steps are two's complement
//!
//! A downward ramp uses a two's-complement `step`, matching
//! [`super::frequency_composer`]: at a fixed width `x + (-y)` and
//! `x - y` are the same bits, so the accumulator needs no signed type
//! and no separate direction flag. [`ramp_step`] produces the right
//! bits for either direction.
//!
//! # Division belongs to the scheduler
//!
//! `Δword / N` is a division, and division does not belong in a
//! datapath. [`ramp_step`] is a `const fn`, so a scheduled segment's
//! step is computed by rustc and arrives as a constant. The hardware
//! only accumulates.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/nco_ramp.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/nco_ramp.md")]

use rhdl::prelude::*;

use crate::core::dff;

use super::config::{self, PHASE_W};

/// Fractional bits below the frequency word in the ramp accumulator.
pub const FRAC_W: usize = 16;

/// Ramp accumulator width: the word plus its fractional part.
pub const ACC_W: usize = PHASE_W + FRAC_W;

/// Segment-length counter width. 2³² samples is ~34 s at 125 MHz.
pub const CNT_W: usize = 32;

const _: () = assert!(
    ACC_W == 64 && FRAC_W == 16,
    "the kernel writes the fractional shift as the literal 16 because \
     the kernel language wants a literal; if FRAC_W changes, that \
     literal must change with it"
);

/// Per-sample step for a segment, in accumulator units, two's
/// complement.
///
/// `step = ((end_word − start_word) · 2^FRAC_W) / samples`
///
/// A `const fn`, so a scheduled segment costs no hardware divider —
/// see the module docs. Returns the two's-complement bits for a
/// downward ramp, so the caller never handles sign.
pub const fn ramp_step(start_microhertz: u128, end_microhertz: u128, samples: u128) -> u128 {
    let a = config::tuning_word(start_microhertz);
    let b = config::tuning_word(end_microhertz);
    let modulus = 1u128 << ACC_W;
    if b >= a {
        (((b - a) << FRAC_W) / samples) % modulus
    } else {
        let magnitude = (((a - b) << FRAC_W) / samples) % modulus;
        (modulus - magnitude) % modulus
    }
}

/// Scheduled frequency segment: linear ramp or chirp.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct FrequencyRamp {
    /// Frequency word with [`FRAC_W`] fractional bits below it.
    acc: dff::DFF<Bits<ACC_W>>,
    /// Per-sample increment, two's complement.
    step: dff::DFF<Bits<ACC_W>>,
    /// Samples left in the segment.
    remaining: dff::DFF<Bits<CNT_W>>,
    /// Endpoint, loaded exactly on the final sample.
    target: dff::DFF<Bits<PHASE_W>>,
    /// A segment is in progress.
    running: dff::DFF<bool>,
}

/// Inputs to [`FrequencyRamp`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// Load the segment described by the other fields this cycle.
    ///
    /// Takes precedence over an in-progress segment, so a scheduler may
    /// retarget without first waiting for `done`.
    pub load: bool,
    /// Frequency word the segment starts from.
    pub start_word: Bits<PHASE_W>,
    /// Frequency word the segment ends on, **exactly** — see the
    /// snapping note in the module docs.
    pub end_word: Bits<PHASE_W>,
    /// Per-sample increment from [`ramp_step`], two's complement.
    pub step: Bits<ACC_W>,
    /// Segment length in samples.
    pub samples: Bits<CNT_W>,
}

/// Outputs from [`FrequencyRamp`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// The frequency word this sample. Feed to
    /// [`super::frequency_composer`]'s `scheduled_offset` or `master`.
    pub word: Bits<PHASE_W>,
    /// A segment is in progress.
    pub running: bool,
    /// One-cycle pulse on the sample the segment completes.
    pub done: bool,
}

impl SynchronousIO for FrequencyRamp {
    type I = In;
    type O = Out;
    type Kernel = frequency_ramp_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn frequency_ramp_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    d.acc = q.acc;
    d.step = q.step;
    d.remaining = q.remaining;
    d.target = q.target;
    d.running = q.running;

    let mut done = false;

    if q.running {
        if q.remaining == bits::<CNT_W>(1) {
            // Final sample: load the endpoint rather than stepping to
            // it, so rounding in `step` cannot leave the segment ending
            // at an almost-right frequency.
            d.acc = q.target.resize::<ACC_W>() << 16;
            d.remaining = bits::<CNT_W>(0);
            d.running = false;
            done = true;
        } else {
            d.acc = q.acc + q.step;
            d.remaining = q.remaining - bits::<CNT_W>(1);
        }
    }

    // Load wins over an in-progress segment.
    if i.load {
        d.acc = i.start_word.resize::<ACC_W>() << 16;
        d.step = i.step;
        d.remaining = i.samples;
        d.target = i.end_word;
        d.running = true;
        done = false;
    }

    let o = Out {
        word: (q.acc >> 16).resize::<PHASE_W>(),
        running: q.running,
        done,
    };

    if cr.reset.any() {
        d.acc = bits::<ACC_W>(0);
        d.step = bits::<ACC_W>(0);
        d.remaining = bits::<CNT_W>(0);
        d.target = bits::<PHASE_W>(0);
        d.running = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    const UHZ: u128 = 1_000_000;

    fn idle() -> In {
        In {
            load: false,
            start_word: bits::<PHASE_W>(0),
            end_word: bits::<PHASE_W>(0),
            step: bits::<ACC_W>(0),
            samples: bits::<CNT_W>(0),
        }
    }

    fn load(start: u128, end: u128, step: u128, samples: u128) -> In {
        In {
            load: true,
            start_word: bits::<PHASE_W>(start),
            end_word: bits::<PHASE_W>(end),
            step: bits::<ACC_W>(step),
            samples: bits::<CNT_W>(samples),
        }
    }

    fn run(seq: Vec<In>) -> Vec<Out> {
        let uut = FrequencyRamp::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = FrequencyRamp::default();
    }

    /// **The test this widget exists for.**
    ///
    /// A ramp whose per-sample step is *less than one LSB* of the
    /// frequency word must still advance. An integer accumulator rounds
    /// such a step to zero and emits a flat line while reporting
    /// success — which is what a 1 Hz-per-second sweep looks like at
    /// 125 MHz (0.018 LSB/sample).
    ///
    /// Verified able to fail: dropping the fractional bits — stepping
    /// `word` directly instead of `acc` — leaves the output constant
    /// and this assertion reports it.
    #[test]
    fn a_ramp_slower_than_one_lsb_per_sample_still_moves() {
        // Half an LSB per sample: 2^(FRAC_W-1).
        let half_lsb = 1u128 << (FRAC_W - 1);
        let start = 1_000_000u128;
        let mut seq = vec![load(start, start + 48, half_lsb, 200)];
        seq.extend(std::iter::repeat_n(idle(), 120));
        let out = run(seq);

        let words: Vec<u128> = out.iter().skip(3).map(|o| o.word.raw()).collect();
        let first = words[0];
        let last = *words.last().unwrap();
        assert!(
            last > first,
            "a sub-LSB step produced a flat ramp: stayed at {first} for the \
             whole run. An integer accumulator does exactly this."
        );
        // Half an LSB per sample over ~118 samples is ~59 LSB.
        assert!(
            (40..80).contains(&(last - first)),
            "expected roughly half an LSB per sample; moved {} over {} samples",
            last - first,
            words.len()
        );
        // And it must be monotonic -- no wobble from the fractional part.
        assert!(
            words.windows(2).all(|w| w[1] >= w[0]),
            "an upward ramp went backwards somewhere: {words:?}"
        );
    }

    /// The segment ends on `end_word` exactly, whatever `step` rounded
    /// to.
    #[test]
    fn the_endpoint_is_exact() {
        // A step deliberately chosen not to divide evenly.
        let start = 1_000u128;
        let end = 9_999u128;
        let samples = 7u128;
        let step = ramp_step(0, 0, 1); // unused magnitude; force rounding below
        let _ = step;
        let bad_step = 12_345u128; // nothing like (end-start)<<16 / 7
        let mut seq = vec![load(start, end, bad_step, samples)];
        seq.extend(std::iter::repeat_n(idle(), 12));
        let out = run(seq);

        let done_at = out
            .iter()
            .position(|o| o.done)
            .expect("the segment never reported done");
        // The snapped value appears on the cycle after `done`.
        let landed = out[done_at + 1].word.raw();
        assert_eq!(
            landed, end,
            "the segment must land exactly on end_word regardless of step rounding"
        );
    }

    /// A downward ramp descends, using a two's-complement step.
    #[test]
    fn a_downward_ramp_descends() {
        let start_hz = 12_000_000u128 * UHZ;
        let end_hz = 11_000_000u128 * UHZ;
        let samples = 64u128;
        let step = ramp_step(start_hz, end_hz, samples);
        assert!(
            step > (1u128 << (ACC_W - 1)),
            "a downward step must be a large two's-complement value, got {step}"
        );
        let mut seq = vec![load(
            config::tuning_word(start_hz),
            config::tuning_word(end_hz),
            step,
            samples,
        )];
        seq.extend(std::iter::repeat_n(idle(), 80));
        let out = run(seq);
        let words: Vec<u128> = out.iter().skip(3).take(60).map(|o| o.word.raw()).collect();
        assert!(
            words.windows(2).all(|w| w[1] <= w[0]),
            "a downward ramp went up somewhere"
        );
        assert!(
            words[0] > *words.last().unwrap(),
            "the ramp did not descend"
        );
    }

    /// [`ramp_step`] agrees with the arithmetic it claims, both ways.
    #[test]
    fn ramp_step_matches_the_definition() {
        let a = 10_000_000u128 * UHZ;
        let b = 10_100_000u128 * UHZ;
        let n = 1000u128;
        let up = ramp_step(a, b, n);
        let want = ((config::tuning_word(b) - config::tuning_word(a)) << FRAC_W) / n;
        assert_eq!(up, want);

        let down = ramp_step(b, a, n);
        assert_eq!(
            (up + down) % (1u128 << ACC_W),
            0,
            "up and down steps must be two's-complement negations"
        );
    }

    /// A new segment preempts one in progress, so a scheduler can
    /// retarget without waiting for `done`.
    #[test]
    fn load_preempts_a_running_segment() {
        let mut seq = vec![load(1_000, 2_000, 1 << FRAC_W, 1000)];
        seq.extend(std::iter::repeat_n(idle(), 4));
        seq.push(load(50_000, 50_010, 1 << FRAC_W, 8));
        seq.extend(std::iter::repeat_n(idle(), 12));
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.word.raw() >= 50_000),
            "the second segment never took effect"
        );
        assert!(
            out.iter().any(|o| o.done),
            "the preempting segment never completed"
        );
    }

    /// Reset clears the segment.
    #[test]
    fn kernel_reset_clears_the_segment() {
        let q = Q {
            acc: bits::<ACC_W>(12345),
            step: bits::<ACC_W>(99),
            remaining: bits::<CNT_W>(7),
            target: bits::<PHASE_W>(4242),
            running: true,
        };
        let cr = clock_reset(clock(false), reset(true));
        let (_o, d) = frequency_ramp_kernel(cr, idle(), q);
        assert_eq!(d.acc.raw(), 0);
        assert_eq!(d.remaining.raw(), 0);
        assert!(!d.running);
    }

    /// Tier 3 — HDL emission snapshot (shape only; the datapath is 64
    /// bits wide and the full text is dominated by register decls).
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = FrequencyRamp::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_acc
            module top_step
            module top_remaining
            module top_target
            module top_running"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    fn hdl_stimulus() -> Vec<In> {
        let mut seq = vec![load(1_000_000, 1_000_500, 1 << FRAC_W, 24)];
        seq.extend(std::iter::repeat_n(idle(), 32));
        seq
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_frequency_ramp_hdl_works() -> miette::Result<()> {
        let uut = FrequencyRamp::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_frequency_ramp_trace() -> miette::Result<()> {
        let uut = FrequencyRamp::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("frequency_ramp");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["04048d53af67a281b78c083c7262326343ad09a3f5670c451b053ccbef35cea7"];
        let digest = vcd.dump_to_file(root.join("frequency_ramp.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
