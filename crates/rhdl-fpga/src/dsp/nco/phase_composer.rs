#![warn(missing_docs)]
//! `PhaseComposer` — sums the layered phase terms into one
//! `phase_offset` for [`super::phase_accumulator::PhaseAccumulator`].
//!
//! §8.2 of the instrument architecture lists the phase terms that must
//! compose: experiment/pulse phase, transmit or receive frame phase,
//! channel calibration, a fine-time-equivalent adjustment, and a user
//! or diagnostic trim. This is the adder tree that combines them.
//!
//! It deliberately carries **no invariant of its own**. The property
//! that matters — that an offset perturbs the output and never the
//! master trajectory — belongs to the accumulator, and is provable
//! there on a widget with one register. Keeping the composition
//! separate is what keeps that proof small.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+PhaseComposer+-------+
 [W]  |                       |
+---->+ pulse                 |
 [W]  |                       |
+---->+ frame                 | [W]
 [W]  |          phase_offset +----->
+---->+ calibration           |
 [W]  |                       |
+---->+ fine_time             |
 [W]  |                       |
+---->+ trim                  |
      +-----------------------+
")]
//!
//! # Wrapping is the arithmetic, not an overflow
//!
//! Every term is `Bits<PHASE_W>` and the sum wraps modulo `2^PHASE_W`.
//! That is correct rather than merely tolerated: phase *is* modulo 2π,
//! and `2^PHASE_W` is one full turn. There is no overflow condition to
//! detect and nothing to saturate.
//!
//! This is the opposite of the amplitude domain, where wrapping is
//! catastrophic — see [`super::sin_cos_linear_interp`], where a one-LSB
//! wrap costs up to 96 dB of SFDR. The two domains look similar and
//! behave oppositely.
//!
//! # Signed terms need no signed type
//!
//! A phase term that should *retard* the output is simply its two's
//! complement, because modulo arithmetic makes `x + (-y)` and
//! `x - y` the same bits at the same width. So the terms stay
//! `Bits<PHASE_W>` and a caller that thinks in signed radians converts
//! once, at the edge. Mixing `SignedBits` in here would buy no
//! type-safety — addition is sign-agnostic — while adding conversions
//! at every call site.
//!
//! # Latency
//!
//! The sum is registered: [`PhaseComposer::LATENCY`] is **1 cycle**.
//!
//! Registered rather than combinational on purpose. At 48 bits and
//! 125 MHz a five-term adder tree is a long carry chain, and §8.4 asks
//! only that the latency be *known* so the scheduler can apply a change
//! that many clocks early — not that it be zero. A stated cycle is
//! cheaper to schedule around than an unstated timing failure.

//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/nco_phase_composer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/nco_phase_composer.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Sums the layered phase terms into a single `phase_offset`.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct PhaseComposer<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Registers the composed sum, so the widget's latency is one
    /// cycle and the carry chain does not extend into the accumulator.
    sum: dff::DFF<Bits<PHASE_W>>,
}

impl<const PHASE_W: usize> PhaseComposer<PHASE_W>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Cycles from a term changing to that change appearing on
    /// `phase_offset`.
    ///
    /// A `usize` associated constant, so the scheduler's arithmetic is
    /// evaluated by rustc and costs nothing in the emitted RTL. See
    /// [`super::latency`].
    pub const LATENCY: usize = 1;
}

/// The layered phase terms of §8.2.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Experiment or pulse phase — the phase the sequence asks for.
    pub pulse: Bits<PHASE_W>,
    /// Transmit or receive frame phase.
    pub frame: Bits<PHASE_W>,
    /// Per-channel calibration, constant for a given hardware setup.
    pub calibration: Bits<PHASE_W>,
    /// Fine-time-equivalent adjustment: sub-sample delay expressed as
    /// phase, which is exact for a single tone.
    pub fine_time: Bits<PHASE_W>,
    /// User or diagnostic trim.
    pub trim: Bits<PHASE_W>,
}

impl<const PHASE_W: usize> SynchronousIO for PhaseComposer<PHASE_W>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    type I = In<PHASE_W>;
    type O = Bits<PHASE_W>;
    type Kernel = phase_composer_kernel<PHASE_W>;
}

#[kernel]
#[doc(hidden)]
pub fn phase_composer_kernel<const PHASE_W: usize>(
    cr: ClockReset,
    i: In<PHASE_W>,
    q: Q<PHASE_W>,
) -> (Bits<PHASE_W>, D<PHASE_W>)
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    let mut d = D::<PHASE_W>::dont_care();

    // Modulo 2^PHASE_W throughout; see the module docs on why wrapping
    // is the arithmetic here rather than an error condition.
    d.sum = i.pulse + i.frame + i.calibration + i.fine_time + i.trim;
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

    fn terms(pulse: u128, frame: u128, cal: u128, fine: u128, trim: u128) -> In<W> {
        In::<W> {
            pulse: bits::<W>(pulse),
            frame: bits::<W>(frame),
            calibration: bits::<W>(cal),
            fine_time: bits::<W>(fine),
            trim: bits::<W>(trim),
        }
    }

    fn run(seq: Vec<In<W>>) -> Vec<u128> {
        let uut = PhaseComposer::<W>::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.raw())
            .collect()
    }

    #[test]
    fn default_construction() {
        let _uut = PhaseComposer::<W>::default();
        let _uut48 = PhaseComposer::<48>::default();
    }

    /// Tier 1 — every term reaches the sum, with the right weight.
    ///
    /// Each term is given a distinct power of two so a dropped or
    /// duplicated term shows up as a specific missing bit rather than
    /// as an arithmetic coincidence. Summing five equal values would
    /// pass even if the kernel added `pulse` five times.
    #[test]
    fn kernel_sums_every_term_exactly_once() {
        let q = Q::<W> { sum: bits::<W>(0) };
        let (_o, d) = phase_composer_kernel::<W>(ClockReset::dont_care(), terms(1, 2, 4, 8, 16), q);
        assert_eq!(
            d.sum.raw(),
            31,
            "each term must contribute exactly its own bit"
        );
    }

    /// Tier 1 — reset drives the register to zero.
    #[test]
    fn kernel_reset_zeroes_the_sum() {
        let q = Q::<W> {
            sum: bits::<W>(0xDEAD),
        };
        let cr = clock_reset(clock(false), reset(true));
        let (_o, d) = phase_composer_kernel::<W>(cr, terms(1, 2, 4, 8, 16), q);
        assert_eq!(d.sum.raw(), 0);
    }

    /// Tier 1 — the sum wraps modulo `2^W`, which is one full turn.
    ///
    /// Not an overflow to be detected: phase is modulo 2π, so wrapping
    /// is the arithmetic. Contrast `sin_cos_linear_interp`, where the
    /// same wrap in the amplitude domain costs up to 96 dB of SFDR.
    #[test]
    fn kernel_wraps_a_full_turn() {
        let full = 1u128 << W;
        let q = Q::<W> { sum: bits::<W>(0) };
        let (_o, d) =
            phase_composer_kernel::<W>(ClockReset::dont_care(), terms(full - 1, 1, 0, 0, 0), q);
        assert_eq!(d.sum.raw(), 0, "a full turn is indistinguishable from none");
    }

    /// Tier 1 — a retarding term is its two's complement, so no signed
    /// type is needed. This is the claim the module docs make.
    #[test]
    fn kernel_subtracts_via_twos_complement() {
        let full = 1u128 << W;
        let q = Q::<W> { sum: bits::<W>(0) };
        let (_o, d) = phase_composer_kernel::<W>(
            ClockReset::dont_care(),
            terms(1000, full - 400, 0, 0, 0),
            q,
        );
        assert_eq!(d.sum.raw(), 600, "1000 + (-400) must be 600");
    }

    /// Tier 2 — the composed offset appears on the output, registered.
    #[test]
    fn composes_through_simulation() {
        let out = run(vec![terms(0, 0, 0, 0, 0); 3]
            .into_iter()
            .chain(vec![terms(100, 20, 3, 0, 0); 4])
            .collect());
        assert!(
            out.iter().any(|v| *v == 123),
            "the composed sum 123 never appeared: {out:?}"
        );
        assert!(
            out.iter().any(|v| *v == 0),
            "the pre-step value never appeared, so the test cannot tell a \
             working composer from one stuck at 123: {out:?}"
        );
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Captured at `W = 8` rather than the `W = 32` the behavioural
    /// tests use: the structure is identical at any width and the
    /// snapshot stays readable, which is the only reason to prefer a
    /// snapshot over a digest.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = PhaseComposer::<8>::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [39:0] i, output wire [7:0] o);
               wire [15:0] od;
               wire [7:0] d;
               wire [7:0] q;
               assign o = od[7:0];
               top_sum c0(.clock_reset(clock_reset), .i(d[7:0]), .o(q[7:0]));
               assign d = od[15:8];
               assign od = kernel_phase_composer_kernel(clock_reset, i, q);
               function [15:0] kernel_phase_composer_kernel(input reg [1:0] arg_0, input reg [39:0] arg_1, input reg [7:0] arg_2);
                     reg [7:0] r0;
                     reg [39:0] r1;
                     reg [7:0] r2;
                     reg [7:0] r3;
                     reg [7:0] r4;
                     reg [7:0] r5;
                     reg [7:0] r6;
                     reg [7:0] r7;
                     reg [7:0] r8;
                     reg [7:0] r9;
                     // d
                     reg [7:0] r10;
                     reg [7:0] r11;
                     reg [0:0] r12;
                     reg [1:0] r13;
                     reg [0:0] r14;
                     // d
                     reg [7:0] r15;
                     // d
                     reg [7:0] r16;
                     reg [15:0] r17;
                     localparam l0 = 8'bXXXXXXXX;
                     localparam l1 = 8'b00000000;
                     begin
                        r13 = arg_0;
                        r1 = arg_1;
                        r11 = arg_2;
                        r0 = r1[7:0];
                        r2 = r1[15:8];
                        r3 = r0 + r2;
                        r4 = r1[23:16];
                        r5 = r3 + r4;
                        r6 = r1[31:24];
                        r7 = r5 + r6;
                        r8 = r1[39:32];
                        r9 = r7 + r8;
                        r10 = l0;
                        r10[7:0] = r9;
                        r12 = r13[1:1];
                        r14 = |r12;
                        r15 = r10;
                        r15[7:0] = l1;
                        r16 = r14 ? r15 : r10;
                        r17 = {r16, r11};
                        kernel_phase_composer_kernel = r17;
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
            .map(|k| terms(k * 7, k * 13, 5, if k > 8 { 1 << 20 } else { 0 }, 0))
            .collect()
    }

    /// Tier 4 — the emitted Verilog agrees with the Rust simulation,
    /// through both the RTL and the NTL paths.
    #[test]
    fn test_phase_composer_hdl_works() -> miette::Result<()> {
        let uut = PhaseComposer::<W>::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_phase_composer_trace() -> miette::Result<()> {
        let uut = PhaseComposer::<W>::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("phase_composer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["811efa19e099669fc83c431b94a3ff51f07a977bf04be4f5f737ea54479379a1"];
        let digest = vcd.dump_to_file(root.join("phase_composer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
