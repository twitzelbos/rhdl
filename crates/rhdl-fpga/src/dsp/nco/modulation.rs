#![warn(missing_docs)]
//! `ModulationInput` — §8.6 sample-synchronous modulation stream.
//!
//! > "A sample-synchronous modulation input is essential for
//! > zero-order eddy-current compensation and other dynamic
//! > corrections. The modulation stream contributes directly to the
//! > frequency word on every NCO update."
//!
//! §8.6 does not ask for a port; it asks for a **contract**, and lists
//! six things it must define. This module is that contract, with each
//! clause answered below and tested.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+ModulationInput+----+
      |                      | [48]
+---->+ stream          word +----->
      |                      |
      |               absent +----->
      |                      |
      |                stale +----->
      +----------------------+
")]
//!
//! # 1. Numeric units and scaling
//!
//! A sample is [`MOD_W`] = 16 bits, two's complement, interpreted as a
//! **frequency deviation**. Full scale maps to
//! [`full_scale_deviation_microhertz`] via a left shift of
//! [`SCALE_SHIFT`] = 16 into frequency-word units:
//!
//! ```text
//! word_contribution = sign_extend(sample) << 16
//! ```
//!
//! A shift rather than a multiply, so the scaling is exact and costs no
//! DSP slice. The resulting full-scale deviation is ±955 Hz at 125 MHz
//! with a 48-bit word — the right order for zero-order eddy-current
//! compensation, whose corrections are Hz to hundreds of Hz.
//!
//! # 2. Signed range and saturation behaviour
//!
//! **The declared range is the type.** A 16-bit two's-complement sample
//! cannot exceed ±full scale, so there is no runtime clamp and none is
//! needed — the saturation behaviour is enforced at the boundary by the
//! width, not by logic that could be wrong.
//!
//! The contribution is two's complement, matching
//! [`super::frequency_composer`]: at a fixed width `x + (-y)` and
//! `x - y` are the same bits, so a downward correction needs no signed
//! type and no direction flag.
//!
//! The sum in the composer is *modulo* `2^48`. With deviation bounded
//! to ±955 Hz and a master frequency in the MHz, the sum cannot
//! approach the wrap point, so wrapping is unreachable rather than
//! suppressed. That is a **precondition on the master frequency**, not
//! a property of this widget:
//! `the_contribution_cannot_wrap_a_sane_master` states it as a test.
//!
//! # 3. Sample rate and interpolation
//!
//! **Same rate as the NCO, one sample per clock. No interpolation.**
//!
//! That is the contract, not a limitation deferred: a compensation
//! waveform is scheduled against the same global timebase as the RF and
//! gradient activity, so it is generated at the sample rate by
//! construction. A differing rate would require an interpolator whose
//! own latency and numerical behaviour would then need defining —
//! §8.6's own standard — and that belongs in a resampling widget
//! upstream, not hidden inside this one.
//!
//! # 4. Latency
//!
//! Registered: one cycle from sample to `word`. Composed with the rest
//! of the chain in [`super::latency`] as
//! [`MODULATION_CONTROL`](super::latency::MODULATION_CONTROL), and
//! measured there rather than asserted.
//!
//! # 5. Behaviour when the stream is absent or invalid
//!
//! An absent sample (`None`) contributes **zero**, and raises `absent`.
//!
//! Zero rather than hold-last, deliberately. A compensation value is
//! *specific to a moment*: eddy-current decay is a function of time
//! since the gradient event, so a held-over correction is not a stale
//! approximation of the right answer, it is a confidently wrong one
//! that persists indefinitely. Reverting to the uncorrected frequency
//! is the conservative failure, and the step it introduces is visible
//! rather than silent.
//!
//! `stale` additionally latches once the stream has been absent for a
//! full sample after having been present, distinguishing "never
//! started" from "stopped mid-experiment" — the second is a fault, the
//! first is just idle.
//!
//! # A codegen defect this widget works around
//!
//! `SignedBits::resize` is the natural way to widen the sample, and it
//! is **wrong here**. Extracted from an `Option` payload it emits
//! `{{32{1'b0}}, r7}` — zero extension — while the Rust simulator
//! sign-extends. Tiers 1 and 2 therefore pass and the `iverilog`
//! round-trip fails, which is how it was caught. The same operation on
//! a *direct* signed input emits `$signed({{30{r38[17]}}, r38})`
//! correctly, so RHDL can do it; something about extraction from an
//! aggregate loses the signedness.
//!
//! This is the same family as the single-field `q`-bundle defect in
//! `tests/signed_literal_comparison.rs`, and is filed as a follow-up
//! rather than fixed here — it is compiler work and belongs in its own
//! PR per CLAUDE.md §11.1.
//!
//! The workaround is explicit sign extension using bit operations only,
//! which does not depend on the operand's declared signedness and is
//! therefore correct either way.
//!
//! # 6. Alignment with pulse and gradient waveforms
//!
//! Not this widget's job, and it cannot be. Alignment is achieved by
//! the scheduler issuing the compensation waveform at the correct lead
//! time using the latency constants, exactly as for phase and frequency
//! control. What this widget provides is **detectability**: `absent`
//! and `stale` make a misaligned or truncated stream visible instead of
//! silently degrading the correction.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/nco_modulation.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/nco_modulation.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::rcstream::bus::Item;

use super::config::{self, PHASE_W};

/// Modulation sample width, two's complement.
pub const MOD_W: usize = 16;

/// Left shift from a modulation sample into frequency-word units.
pub const SCALE_SHIFT: usize = 16;

const _: () = assert!(
    SCALE_SHIFT == 16 && MOD_W == 16,
    "the kernel writes the scaling shift as the literal 16 because the \
     kernel language wants a literal; if SCALE_SHIFT changes, that \
     literal must change with it"
);

/// Frequency deviation, in µHz, that a full-scale modulation sample
/// commands.
///
/// A `const fn`, so the range this widget accepts is a compile-time
/// fact rather than a comment. See §1 of the module docs.
pub const fn full_scale_deviation_microhertz() -> u128 {
    let full_scale_word = (1u128 << (MOD_W - 1)) << SCALE_SHIFT;
    config::frequency_microhertz(full_scale_word)
}

/// §8.6 sample-synchronous modulation input.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct ModulationInput {
    /// Registered frequency-word contribution.
    word: dff::DFF<Bits<PHASE_W>>,
    /// The stream has been present at least once.
    started: dff::DFF<bool>,
    /// The stream stopped after having started.
    stale: dff::DFF<bool>,
}

/// Inputs to [`ModulationInput`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// The modulation stream. `None` is an absent sample — see §5.
    ///
    /// `F = ()`: nothing is framed in the timed domain.
    pub stream: Option<Item<SignedBits<MOD_W>, ()>>,
}

/// Outputs from [`ModulationInput`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out {
    /// Two's-complement contribution to the frequency word. Feed to
    /// [`super::frequency_composer`]'s `modulation` term.
    pub word: Bits<PHASE_W>,
    /// No sample this cycle; the contribution is zero.
    pub absent: bool,
    /// The stream stopped after having started — a fault, as distinct
    /// from never having started.
    pub stale: bool,
}

impl SynchronousIO for ModulationInput {
    type I = In;
    type O = Out;
    type Kernel = modulation_input_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn modulation_input_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    d.started = q.started;
    d.stale = q.stale;

    let mut absent = true;
    let mut contribution = bits::<PHASE_W>(0);

    match i.stream {
        Some(item) => {
            absent = false;
            d.started = true;

            // Sign-extend explicitly, with bit operations only.
            //
            // `SignedBits::resize` is the natural way to write this and
            // is WRONG here: extracted from an `Option` payload it
            // emits `{{32{1'b0}}, r7}` -- zero extension -- while the
            // Rust simulator sign-extends, so Tiers 1 and 2 pass and
            // the iverilog round-trip fails. See the note in the module
            // docs; the same operation on a direct signed input emits
            // `$signed({{30{r38[17]}}, r38})` correctly.
            //
            // Testing the sign bit with AND and comparing to zero does
            // not depend on the operand's declared signedness, so this
            // form is correct either way.
            let raw = item.data.as_unsigned();
            let shifted = raw.resize::<PHASE_W>() << 16;
            let negative = (raw & bits::<MOD_W>(0x8000)) != bits::<MOD_W>(0);
            // Two's complement of the scaled value: adding 2^48 - 2^32
            // is subtracting 2^32, which is the sign extension of a
            // 16-bit negative shifted left by 16.
            let sign_fill = if negative {
                bits::<PHASE_W>(281_470_681_743_360)
            } else {
                bits::<PHASE_W>(0)
            };
            contribution = shifted + sign_fill;
        }
        None => {
            // Zero, not hold-last: a compensation value is specific to
            // a moment, so a held-over one is confidently wrong rather
            // than merely stale. See §5.
            if q.started {
                d.stale = true;
            }
        }
    }

    d.word = contribution;

    let o = Out {
        word: q.word,
        absent,
        stale: q.stale,
    };

    if cr.reset.any() {
        d.word = bits::<PHASE_W>(0);
        d.started = false;
        d.stale = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    fn sample(v: i128) -> In {
        In {
            stream: Some(Item::<SignedBits<MOD_W>, ()> {
                data: signed::<MOD_W>(v),
                frame: (),
            }),
        }
    }

    fn gap() -> In {
        In { stream: None }
    }

    fn run(seq: Vec<In>) -> Vec<Out> {
        let uut = ModulationInput::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = ModulationInput::default();
    }

    /// §1 — the declared full-scale deviation is what the scaling
    /// actually produces.
    ///
    /// The docs claim ±955 Hz; a comment that drifts from the shift is
    /// worse than no comment, so it is computed and checked.
    #[test]
    fn full_scale_deviation_is_as_documented() {
        let uhz = full_scale_deviation_microhertz();
        let hz = uhz / 1_000_000;
        assert!(
            (900..1000).contains(&hz),
            "full-scale deviation is {hz} Hz; the module docs claim ~955 Hz"
        );
    }

    /// §1 — a positive sample scales by exactly `2^SCALE_SHIFT`.
    #[test]
    fn scaling_is_an_exact_shift() {
        let out = run(vec![sample(3); 4]);
        let w = out.iter().map(|o| o.word.raw()).max().unwrap();
        assert_eq!(w, 3 << SCALE_SHIFT);
    }

    /// §2 — a negative deviation is its two's complement, so it lowers
    /// the frequency when added to the composer's sum.
    #[test]
    fn a_negative_sample_is_twos_complement() {
        let out = run(vec![sample(-3); 4]);
        let w = out
            .iter()
            .map(|o| o.word.raw())
            .find(|w| *w != 0)
            .expect("no contribution appeared");
        let modulus = 1u128 << PHASE_W;
        assert_eq!(w, modulus - (3 << SCALE_SHIFT));
        // And adding it to a master really does subtract.
        let master = 1_000_000_000u128;
        assert_eq!((master + w) % modulus, master - (3 << SCALE_SHIFT));
    }

    /// §2 — the bound is the type, so a full-scale sample cannot push a
    /// sane master frequency anywhere near the wrap point.
    ///
    /// This is the precondition the module docs state: wrapping is
    /// unreachable rather than suppressed, and that depends on the
    /// master being a sane frequency, which is not this widget's
    /// property to guarantee.
    #[test]
    fn the_contribution_cannot_wrap_a_sane_master() {
        let full = (1u128 << (MOD_W - 1)) << SCALE_SHIFT;
        let modulus = 1u128 << PHASE_W;
        // 10 MHz, the low end of the first application's tuning range.
        let master = config::tuning_word(10_000_000 * 1_000_000);
        assert!(
            master > full,
            "a full-scale downward deviation must not take a 10 MHz \
             master below zero"
        );
        assert!(
            master + full < modulus,
            "a full-scale upward deviation must not wrap"
        );
    }

    /// §5 — an absent sample contributes zero and says so.
    #[test]
    fn an_absent_sample_contributes_zero_and_is_reported() {
        let mut seq = vec![sample(500); 4];
        seq.extend(vec![gap(); 4]);
        let out = run(seq);
        assert!(out.iter().any(|o| o.absent), "a gap was never reported");
        // The contribution returns to zero rather than holding the last
        // value -- a compensation value is specific to a moment.
        let tail = out.last().unwrap();
        assert_eq!(tail.word.raw(), 0, "the contribution was held over a gap");
    }

    /// §5 — `stale` distinguishes "stopped mid-experiment" from "never
    /// started".
    ///
    /// Without the distinction an idle stream before the first sample
    /// looks identical to one that died, and only the second is a
    /// fault.
    #[test]
    fn stale_separates_a_dead_stream_from_an_idle_one() {
        // Never started: gaps only.
        let never = run(vec![gap(); 8]);
        assert!(
            never.iter().all(|o| !o.stale),
            "an idle stream that never started must not report stale"
        );

        // Started, then stopped.
        let mut seq = vec![sample(100); 3];
        seq.extend(vec![gap(); 6]);
        let died = run(seq);
        assert!(
            died.iter().any(|o| o.stale),
            "a stream that stopped after starting must report stale"
        );
    }

    /// Reset clears the contribution and the fault latch.
    #[test]
    fn kernel_reset_clears_everything() {
        let q = Q {
            word: bits::<PHASE_W>(12345),
            started: true,
            stale: true,
        };
        let cr = clock_reset(clock(false), reset(true));
        let (_o, d) = modulation_input_kernel(cr, gap(), q);
        assert_eq!(d.word.raw(), 0);
        assert!(!d.started);
        assert!(!d.stale);
    }

    /// Tier 3 — HDL emission snapshot.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = ModulationInput::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let shape = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .map(|l| l.split('(').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top
            module top_word
            module top_started
            module top_stale"#]];
        expect.assert_eq(&shape);
        Ok(())
    }

    fn hdl_stimulus() -> Vec<In> {
        let mut seq: Vec<In> = (0..16i128).map(|k| sample((k - 8) * 400)).collect();
        seq.extend(vec![gap(); 6]);
        seq.extend((0..8i128).map(|k| sample(k * 100)));
        seq
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_modulation_input_hdl_works() -> miette::Result<()> {
        let uut = ModulationInput::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_modulation_input_trace() -> miette::Result<()> {
        let uut = ModulationInput::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("modulation_input");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["fef4e3a6b6c48af173560e7df71645a7bc348a6d1ca646608998324b4947bb04"];
        let digest = vcd.dump_to_file(root.join("modulation_input.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
