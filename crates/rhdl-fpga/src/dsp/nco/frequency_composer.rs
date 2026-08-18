#![warn(missing_docs)]
//! `FrequencyComposer` — sums the frequency terms into one
//! `frequency_word` for [`super::phase_accumulator::PhaseAccumulator`].
//!
//! §8.3 of the instrument architecture:
//!
//! ```text
//! frequency_word[n] = master + scheduled_offset[n] + modulation[n] + calibration[n]
//! ```
//!
//! The master frequency establishes the nominal rotating frame;
//! scheduled offsets change the phase slope for defined intervals.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+FrequencyComposer+-----+
 [W]  |                         |
+---->+ master                  |
 [W]  |                         | [W]
+---->+ scheduled_offset        |
 [W]  |          frequency_word +----->
+---->+ modulation              |
 [W]  |                         |
+---->+ calibration             |
      +-------------------------+
")]
//!
//! # The distinction §8.3 warns must not be confused
//!
//! Removing a frequency offset returns the *slope* to the master
//! frequency. It does **not** erase the phase accumulated while the
//! offset was active. The oscillator resumes at the nominal rate from
//! wherever it has got to, which is phase-continuous and physically
//! what a real rotating frame does.
//!
//! That is a different operation from returning to the phase the
//! oscillator *would* have had if the offset had never been applied.
//! Getting back to that hypothetical trajectory requires the scheduler
//! to apply a compensating **phase** correction — through
//! [`super::phase_composer::PhaseComposer`], not through this one.
//!
//! Both are legitimate; confusing them silently corrupts phase
//! coherence across an averaged experiment. The two paths are separate
//! widgets precisely so the choice has to be made explicitly:
//!
//! | intent | mechanism |
//! |---|---|
//! | resume at nominal rate, keep accumulated phase | remove the offset here |
//! | return to the unmodulated trajectory | remove the offset **and** apply a compensating phase term |
//!
//! Both are pinned by tests, and deliberately not here: the semantics
//! belong to the accumulator, which is where they are provable on a
//! widget with one register.
//! `phase_accumulator::tests::removing_a_frequency_offset_keeps_the_accumulated_phase`
//! pins the first; `removing_an_offset_rejoins_the_untouched_trajectory`
//! pins the second. This widget's own job is only to make sure each
//! term reaches the word, which `kernel_sums_every_term_exactly_once`
//! checks.
//!
//! # Signed terms need no signed type
//!
//! A downward frequency offset is its two's complement: at a fixed
//! width, `x + (-y)` and `x - y` are the same bits, so addition is
//! sign-agnostic. Terms stay `Bits<PHASE_W>` and a caller thinking in
//! signed Hz converts once, at the edge.
//!
//! Note this is *not* true of comparison, which is why `SignedBits`
//! exists at all — but nothing here compares.
//!
//! # Latency
//!
//! The sum is registered: [`FrequencyComposer::LATENCY`] is **1
//! cycle**. Same reasoning as the phase composer — §8.4 requires the
//! latency be known, not zero, and a 48-bit four-term adder tree at
//! 125 MHz is worth a register.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/nco_frequency_composer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/nco_frequency_composer.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Sums the frequency terms into a single `frequency_word`.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct FrequencyComposer<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Registers the composed sum.
    sum: dff::DFF<Bits<PHASE_W>>,
}

impl<const PHASE_W: usize> FrequencyComposer<PHASE_W>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Cycles from a term changing to that change appearing on
    /// `frequency_word`. See [`super::latency`].
    pub const LATENCY: usize = 1;
}

/// The frequency terms of §8.3.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Nominal rotating frame. Normally constant for an experiment.
    pub master: Bits<PHASE_W>,
    /// Scheduled offset, applied for a defined interval.
    pub scheduled_offset: Bits<PHASE_W>,
    /// Sample-synchronous modulation — §8.6's eddy-current
    /// compensation input contributes here.
    pub modulation: Bits<PHASE_W>,
    /// Per-channel calibration.
    pub calibration: Bits<PHASE_W>,
}

impl<const PHASE_W: usize> SynchronousIO for FrequencyComposer<PHASE_W>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    type I = In<PHASE_W>;
    type O = Bits<PHASE_W>;
    type Kernel = frequency_composer_kernel<PHASE_W>;
}

#[kernel]
#[doc(hidden)]
pub fn frequency_composer_kernel<const PHASE_W: usize>(
    cr: ClockReset,
    i: In<PHASE_W>,
    q: Q<PHASE_W>,
) -> (Bits<PHASE_W>, D<PHASE_W>)
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    let mut d = D::<PHASE_W>::dont_care();

    d.sum = i.master + i.scheduled_offset + i.modulation + i.calibration;
    let o = q.sum;

    if cr.reset.any() {
        d.sum = bits::<PHASE_W>(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    const W: usize = 32;

    fn terms(master: u128, offset: u128, modulation: u128, cal: u128) -> In<W> {
        In::<W> {
            master: bits::<W>(master),
            scheduled_offset: bits::<W>(offset),
            modulation: bits::<W>(modulation),
            calibration: bits::<W>(cal),
        }
    }

    #[test]
    fn default_construction() {
        let _uut = FrequencyComposer::<W>::default();
        let _uut48 = FrequencyComposer::<48>::default();
    }

    /// Tier 1 — every term reaches the word, with the right weight.
    ///
    /// Distinct powers of two, so a dropped or duplicated term shows as
    /// a specific missing bit rather than an arithmetic coincidence.
    #[test]
    fn kernel_sums_every_term_exactly_once() {
        let q = Q::<W> { sum: bits::<W>(0) };
        let (_o, d) = frequency_composer_kernel::<W>(ClockReset::dont_care(), terms(1, 2, 4, 8), q);
        assert_eq!(
            d.sum.raw(),
            15,
            "each term must contribute exactly its own bit"
        );
    }

    /// Tier 1 — reset drives the register to zero.
    #[test]
    fn kernel_reset_zeroes_the_word() {
        let q = Q::<W> {
            sum: bits::<W>(0xBEEF),
        };
        let cr = clock_reset(clock(false), reset(true));
        let (_o, d) = frequency_composer_kernel::<W>(cr, terms(1, 2, 4, 8), q);
        assert_eq!(d.sum.raw(), 0);
    }

    /// Tier 1 — a downward offset is its two's complement.
    ///
    /// This is what lets the terms stay unsigned: at a fixed width
    /// `x + (-y)` and `x - y` are the same bits.
    #[test]
    fn kernel_lowers_frequency_via_twos_complement() {
        let full = 1u128 << W;
        let q = Q::<W> { sum: bits::<W>(0) };
        let (_o, d) = frequency_composer_kernel::<W>(
            ClockReset::dont_care(),
            terms(10_000, full - 2_500, 0, 0),
            q,
        );
        assert_eq!(d.sum.raw(), 7_500, "a negative offset must lower the word");
    }

    /// Tier 2 — a scheduled offset raises the word and removing it
    /// returns the word to master, through simulation.
    ///
    /// Note this is about the *word*, not the phase. That removing the
    /// offset does not erase phase already accumulated is the
    /// accumulator's property, pinned by
    /// `phase_accumulator::tests::removing_a_frequency_offset_keeps_the_accumulated_phase`.
    #[test]
    fn scheduled_offset_raises_then_releases_the_word() {
        let uut = FrequencyComposer::<W>::default();
        let seq: Vec<In<W>> = (0..15u128)
            .map(|k| terms(1000, if (5..10).contains(&k) { 250 } else { 0 }, 0, 0))
            .collect();
        let out: Vec<u128> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.raw())
            .collect();
        assert!(
            out.iter().any(|v| *v == 1000),
            "master-only never appeared: {out:?}"
        );
        assert!(
            out.iter().any(|v| *v == 1250),
            "offset never appeared: {out:?}"
        );
        // And it must come back down -- an offset that latches would
        // pass both assertions above.
        let last = out.last().copied().unwrap();
        assert_eq!(last, 1000, "the offset did not release: {out:?}");
    }

    /// Tier 3 — HDL emission snapshot, at `W = 8` for readability.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = FrequencyComposer::<8>::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [31:0] i, output wire [7:0] o);
               wire [15:0] od;
               wire [7:0] d;
               wire [7:0] q;
               assign o = od[7:0];
               top_sum c0(.clock_reset(clock_reset), .i(d[7:0]), .o(q[7:0]));
               assign d = od[15:8];
               assign od = kernel_frequency_composer_kernel(clock_reset, i, q);
               function [15:0] kernel_frequency_composer_kernel(input reg [1:0] arg_0, input reg [31:0] arg_1, input reg [7:0] arg_2);
                     reg [7:0] r0;
                     reg [31:0] r1;
                     reg [7:0] r2;
                     reg [7:0] r3;
                     reg [7:0] r4;
                     reg [7:0] r5;
                     reg [7:0] r6;
                     reg [7:0] r7;
                     // d
                     reg [7:0] r8;
                     reg [7:0] r9;
                     reg [0:0] r10;
                     reg [1:0] r11;
                     reg [0:0] r12;
                     // d
                     reg [7:0] r13;
                     // d
                     reg [7:0] r14;
                     reg [15:0] r15;
                     localparam l0 = 8'bXXXXXXXX;
                     localparam l1 = 8'b00000000;
                     begin
                        r11 = arg_0;
                        r1 = arg_1;
                        r9 = arg_2;
                        r0 = r1[7:0];
                        r2 = r1[15:8];
                        r3 = r0 + r2;
                        r4 = r1[23:16];
                        r5 = r3 + r4;
                        r6 = r1[31:24];
                        r7 = r5 + r6;
                        r8 = l0;
                        r8[7:0] = r7;
                        r10 = r11[1:1];
                        r12 = |r10;
                        r13 = r8;
                        r13[7:0] = l1;
                        r14 = r12 ? r13 : r8;
                        r15 = {r14, r9};
                        kernel_frequency_composer_kernel = r15;
                     end
               endfunction
            endmodule
            module top_sum(input wire [1:0] clock_reset, input wire [7:0] i, output reg [7:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 8'b00000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 8'b00000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    fn hdl_stimulus() -> Vec<In<W>> {
        (0..24u128)
            .map(|k| terms(1000, if (8..16).contains(&k) { 250 } else { 0 }, k * 3, 7))
            .collect()
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation, RTL
    /// and NTL.
    #[test]
    fn test_frequency_composer_hdl_works() -> miette::Result<()> {
        let uut = FrequencyComposer::<W>::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_frequency_composer_trace() -> miette::Result<()> {
        let uut = FrequencyComposer::<W>::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("frequency_composer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["6089b172031c51840fd5f7c5c8c50a95d99563466c274cb387f480c59fdf2161"];
        let digest = vcd
            .dump_to_file(root.join("frequency_composer.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
