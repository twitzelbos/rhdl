#![warn(missing_docs)]
//! `PhaseAccumulator<PHASE_W>` — the free-running master phase of a
//! phase-coherent NCO.
//!
//! Maintains a wide accumulator that advances by a frequency word every
//! cycle and is **never disturbed by phase offsets**. Offsets are added
//! to the *output* only, so applying and later removing one leaves the
//! master trajectory exactly where it would have been.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +-+PhaseAccumulator+--+
 [W]  |                     | [W]
+---->+ frequency_word phase+----->
 [W]  |                     | [W]
+---->+ phase_offset  master+----->
      +---------------------+
")]
//!
//! # Why the master phase must not be reset
//!
//! ```text
//! master[n+1] = master[n] + frequency_word[n]
//! phase[n]    = master[n] + phase_offset[n]
//! ```
//!
//! The accumulator is not reset at pulse or acquisition boundaries.
//! That is what makes phase *coherent*: an experiment repeated after an
//! arbitrary delay sees the phase the free-running oscillator would
//! have had, rather than one that depends on when the last pulse
//! happened to end.
//!
//! A temporary phase offset can therefore be applied and removed
//! without disturbing the master trajectory. **When the offset returns
//! to zero, the output rejoins the phase it would have had if the
//! offset had never been applied** — see
//! `removing_an_offset_rejoins_the_untouched_trajectory`, which is
//! the property this widget exists to guarantee.
//!
//! # Frequency composition is deliberately external
//!
//! Per the architecture note, the frequency word is a sum:
//!
//! ```text
//! frequency_word = master_frequency + scheduled_offset + modulation + calibration
//! ```
//!
//! That sum is a plain adder and carries no invariant of its own, so it
//! lives outside this widget. Keeping the accumulator pure makes the
//! one property that *does* matter — offset independence — testable in
//! isolation.
//!
//! Note the physical consequence of composing frequency this way:
//! removing a frequency offset returns the phase *slope* to the master
//! frequency but does **not** erase the phase accumulated while the
//! offset was active. That is correct and phase-continuous. Returning
//! to the hypothetical unmodulated trajectory is a different operation
//! requiring a compensating phase correction, and must not be confused
//! with it.
//!
//! # Wrapping
//!
//! All arithmetic is modulo `2^PHASE_W`, which *is* phase wrapping —
//! `Bits<N>` wraps rather than saturating, so no explicit modulo is
//! needed and none is emitted.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/nco_phase.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/nco_phase.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Free-running master phase accumulator.
///
/// # Choosing `PHASE_W`
///
/// The accumulator width sets frequency resolution,
/// `Δf = f_clk / 2^PHASE_W`.
///
/// **Quantisation here is not drift.** The accumulator is exact integer
/// arithmetic, so the synthesised frequency is precisely
/// `word · f_clk / 2^W` — a fixed, known, perfectly *repeatable* offset
/// from the requested frequency. It appears as a linear phase ramp,
/// indistinguishable from a small resonance offset: removed by
/// first-order phase correction, and cancelling exactly in phase-cycled
/// differences because every scan uses the same tuning word.
///
/// The binding criterion is therefore resolution against the narrowest
/// linewidth of interest, not accumulated error. At 125 MHz:
///
/// | `PHASE_W` | `Δf` | vs a 0.1 Hz linewidth |
/// |---|---|---|
/// | 32 | 29 mHz | ~30% — defensible but marginal |
/// | 37 | 0.9 mHz | ~1% |
/// | 40 | 114 µHz | ~0.1% |
/// | 48 | 0.44 µHz | negligible |
///
/// 32 bits is probably adequate and is cheap; ~37 is where the question
/// stops being arguable; 40 or 48 is round-number margin on a register
/// costing one carry chain. The choice rests on the narrowest linewidth
/// the instrument must resolve — an application spec, pinned in
/// `deployment_width_resolves_the_narrowest_linewidth` so that
/// changing it tells you the width you now need.
///
/// This is **independent of the phase-to-amplitude stage's addressing
/// width**, which truncates to `P` bits (11–13 for 60–70 dB SFDR — see
/// the module docs). A 48-bit accumulator feeding a 13-bit table
/// discards 35 bits at lookup. `PHASE_W` controls frequency resolution;
/// `P` controls spur performance.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct PhaseAccumulator<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// The master trajectory. Advances every cycle; never perturbed by
    /// a phase offset.
    master: dff::DFF<Bits<PHASE_W>>,
}

impl<const PHASE_W: usize> Default for PhaseAccumulator<PHASE_W>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            master: dff::DFF::new(bits::<PHASE_W>(0)),
        }
    }
}

/// Inputs for [`PhaseAccumulator`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// Added to the master accumulator every cycle. Compose it upstream
    /// from master frequency, scheduled offsets, modulation and
    /// calibration.
    pub frequency_word: Bits<PHASE_W>,
    /// Added to the **output** only. Does not perturb the master
    /// trajectory, so it can be applied and removed freely.
    pub phase_offset: Bits<PHASE_W>,
}

/// Outputs from [`PhaseAccumulator`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const PHASE_W: usize>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    /// `master + phase_offset` — the phase to hand to phase-to-amplitude
    /// conversion.
    pub phase: Bits<PHASE_W>,
    /// The undisturbed master trajectory.
    ///
    /// Exposed so that offset independence is observable as a black-box
    /// property rather than by reaching into internal state, and so a
    /// second consumer (a receive mixer, say) can share one master
    /// phase.
    pub master: Bits<PHASE_W>,
}

impl<const PHASE_W: usize> SynchronousIO for PhaseAccumulator<PHASE_W>
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    type I = In<PHASE_W>;
    type O = Out<PHASE_W>;
    type Kernel = phase_accumulator_kernel<PHASE_W>;
}

#[kernel]
#[doc(hidden)]
pub fn phase_accumulator_kernel<const PHASE_W: usize>(
    cr: ClockReset,
    i: In<PHASE_W>,
    q: Q<PHASE_W>,
) -> (Out<PHASE_W>, D<PHASE_W>)
where
    rhdl::bits::W<PHASE_W>: BitWidth,
{
    let mut d = D::<PHASE_W>::dont_care();
    let mut o = Out::<PHASE_W>::dont_care();

    // The master trajectory advances unconditionally.  Note what is NOT
    // here: `phase_offset` never reaches `d.master`.  That omission is
    // the widget's entire reason for existing.
    d.master = q.master + i.frequency_word;

    o.master = q.master;
    o.phase = q.master + i.phase_offset;

    // Hardware reset only.  This is distinct from a pulse or
    // acquisition boundary, which must NOT reset the accumulator.
    if cr.reset.any() {
        d.master = bits::<PHASE_W>(0);
        o.master = bits::<PHASE_W>(0);
        o.phase = bits::<PHASE_W>(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    const W: usize = 32;

    fn input(freq: u128, offset: u128) -> In<W> {
        In::<W> {
            frequency_word: bits::<W>(freq),
            phase_offset: bits::<W>(offset),
        }
    }

    /// Run the accumulator over a sequence of inputs, returning
    /// `(phase, master)` per cycle.
    fn run(seq: Vec<In<W>>) -> Vec<(u128, u128)> {
        let uut = PhaseAccumulator::<W>::default();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(stream)
            .synchronous_sample()
            .map(|s| (s.output.phase.raw(), s.output.master.raw()))
            .collect()
    }

    /// A short stimulus reused by Tiers 3 to 5, so the snapshot, the
    /// Verilog round-trip and the committed waveform all describe the
    /// same thing.
    ///
    /// The offset steps on and off mid-run: that is the only part of
    /// the input space where `phase` and `master` diverge, so a
    /// stimulus without it would exercise one adder and call it
    /// coverage.
    fn hdl_stimulus() -> Vec<In<W>> {
        (0..32u128)
            .map(|k| input(0x0100_0000, if (8..20).contains(&k) { 1 << 30 } else { 0 }))
            .collect()
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Captured at `W = 8` rather than the `W = 32` the other tests use.
    /// The structure is identical at any width and the snapshot stays
    /// readable, which is the point of having one: a digest would catch
    /// the same regressions and tell a reviewer nothing.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = PhaseAccumulator::<8>::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [15:0] i, output wire [15:0] o);
               wire [23:0] od;
               wire [7:0] d;
               wire [7:0] q;
               assign o = od[15:0];
               top_master c0(.clock_reset(clock_reset), .i(d[7:0]), .o(q[7:0]));
               assign d = od[23:16];
               assign od = kernel_phase_accumulator_kernel(clock_reset, i, q);
               function [23:0] kernel_phase_accumulator_kernel(input reg [1:0] arg_0, input reg [15:0] arg_1, input reg [7:0] arg_2);
                     reg [7:0] r0;
                     reg [7:0] r1;
                     reg [15:0] r2;
                     reg [7:0] r3;
                     // d
                     reg [7:0] r4;
                     // o
                     reg [15:0] r5;
                     reg [7:0] r6;
                     reg [7:0] r7;
                     // o
                     reg [15:0] r8;
                     reg [0:0] r9;
                     reg [1:0] r10;
                     reg [0:0] r11;
                     // d
                     reg [7:0] r12;
                     // o
                     reg [15:0] r13;
                     // o
                     reg [15:0] r14;
                     // d
                     reg [7:0] r15;
                     // o
                     reg [15:0] r16;
                     reg [23:0] r17;
                     localparam l0 = 8'bXXXXXXXX;
                     localparam l1 = 16'bXXXXXXXXXXXXXXXX;
                     localparam l2 = 8'b00000000;
                     localparam l3 = 8'b00000000;
                     localparam l4 = 8'b00000000;
                     begin
                        r10 = arg_0;
                        r2 = arg_1;
                        r0 = arg_2;
                        r1 = r2[7:0];
                        r3 = r0 + r1;
                        r4 = l0;
                        r4[7:0] = r3;
                        r5 = l1;
                        r5[15:8] = r0;
                        r6 = r2[15:8];
                        r7 = r0 + r6;
                        r8 = r5;
                        r8[7:0] = r7;
                        r9 = r10[1:1];
                        r11 = |r9;
                        r12 = r4;
                        r12[7:0] = l2;
                        r13 = r8;
                        r13[15:8] = l3;
                        r14 = r13;
                        r14[7:0] = l4;
                        r15 = r11 ? r12 : r4;
                        r16 = r11 ? r14 : r8;
                        r17 = {r15, r16};
                        kernel_phase_accumulator_kernel = r17;
                     end
               endfunction
            endmodule
            module top_master(input wire [1:0] clock_reset, input wire [7:0] i, output reg [7:0] o);
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

    /// Tier 4 — the emitted Verilog agrees with the Rust simulation
    /// cycle by cycle, through both the RTL and the NTL paths.
    #[test]
    fn test_phase_accumulator_hdl_works() -> miette::Result<()> {
        let uut = PhaseAccumulator::<W>::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn test_phase_accumulator_trace() -> miette::Result<()> {
        let uut = PhaseAccumulator::<W>::default();
        let stream = hdl_stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("phase_accumulator");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["d9003178ad45687171ea1bb6f2231a973d07ad0e5915018981332c8b9ea1aec0"];
        let digest = vcd
            .dump_to_file(root.join("phase_accumulator.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn default_construction() {
        let _a = PhaseAccumulator::<32>::default();
        let _b = PhaseAccumulator::<24>::default();
        let _c = PhaseAccumulator::<48>::default();
    }

    /// **The accumulator-width specification, as a test.**
    ///
    /// Quantisation here is *not* drift. The accumulator is exact
    /// integer arithmetic, so the synthesised frequency is precisely
    /// `word · f_clk / 2^W` — a fixed, known, perfectly repeatable
    /// offset from the requested frequency, producing a linear phase
    /// ramp indistinguishable from a small resonance offset. It is
    /// removed by first-order phase correction and cancels exactly in
    /// phase-cycled differences, because every scan uses the same
    /// tuning word.
    ///
    /// The criterion that does bind is **frequency resolution against
    /// the narrowest linewidth of interest**:
    ///
    /// ```text
    /// Δf = f_clk / 2^PHASE_W   must be ≪ narrowest linewidth
    /// ```
    ///
    /// `NARROWEST_LINEWIDTH_HZ` below is the spec this rests on, and it
    /// is an instrument decision rather than a hardware one. Change it
    /// and this test tells you the width you now need.
    #[test]
    fn deployment_width_resolves_the_narrowest_linewidth() {
        const F_CLK: f64 = 125.0e6;
        const DEPLOYMENT_W: i32 = 48;
        /// The narrowest line the instrument is expected to resolve.
        /// **Assumption — confirm against the application.**
        const NARROWEST_LINEWIDTH_HZ: f64 = 0.1;
        /// Frequency resolution should be a small fraction of that, so
        /// the residual offset is negligible against the line itself.
        const FRACTION_OF_LINEWIDTH: f64 = 0.01;

        let delta_f = |w: i32| F_CLK / 2f64.powi(w);
        let budget = NARROWEST_LINEWIDTH_HZ * FRACTION_OF_LINEWIDTH;

        assert!(
            delta_f(DEPLOYMENT_W) < budget,
            "Δf at {DEPLOYMENT_W} bits is {:.3e} Hz, budget {budget:.3e} Hz",
            delta_f(DEPLOYMENT_W)
        );

        // For the record, and so the trade is visible rather than
        // asserted: 32 bits gives ~29 mHz, which is roughly 30% of a
        // 0.1 Hz linewidth — defensible, but marginal. ~37 bits is
        // where it stops being arguable.
        assert!(
            delta_f(32) > budget,
            "if 32 bits also met this budget the deployment width would be \
             over-specified and should be reduced"
        );
        assert!(
            delta_f(37) < NARROWEST_LINEWIDTH_HZ * 0.02,
            "37 bits should be the point where the margin is comfortable"
        );
    }

    /// The widget works at the deployment width, not just the 32 bits
    /// the other tests use for legibility.
    #[test]
    fn works_at_forty_eight_bits() {
        let q = Q::<48> {
            master: bits::<48>(1 << 40),
        };
        let (o, d) = phase_accumulator_kernel::<48>(
            clock_reset(clock(true), reset(false)),
            In::<48> {
                frequency_word: bits::<48>(12345),
                phase_offset: bits::<48>(1 << 47),
            },
            q,
        );
        assert_eq!(d.master.raw(), (1u128 << 40) + 12345);
        assert_eq!(o.phase.raw(), (1u128 << 40) + (1u128 << 47));
    }

    /// The accumulator advances by the frequency word each cycle.
    #[test]
    fn master_advances_by_the_frequency_word() {
        let q = Q::<W> {
            master: bits::<W>(1000),
        };
        let (o, d) =
            phase_accumulator_kernel::<W>(clock_reset(clock(true), reset(false)), input(7, 0), q);
        assert_eq!(o.master.raw(), 1000);
        assert_eq!(d.master.raw(), 1007);
    }

    /// A phase offset shifts the output and **not** the accumulator.
    #[test]
    fn offset_shifts_output_but_not_master() {
        let q = Q::<W> {
            master: bits::<W>(1000),
        };
        let (o, d) =
            phase_accumulator_kernel::<W>(clock_reset(clock(true), reset(false)), input(7, 500), q);
        assert_eq!(o.phase.raw(), 1500, "offset applies to the output");
        assert_eq!(o.master.raw(), 1000, "master is unshifted");
        assert_eq!(
            d.master.raw(),
            1007,
            "the offset must not enter the accumulator"
        );
    }

    /// **The property this widget exists to guarantee** (architecture
    /// note §8.1).
    ///
    /// Apply a phase offset for a while, then remove it. The master
    /// trajectory must be bit-identical to a run where the offset was
    /// never applied, and the output must rejoin that trajectory the
    /// moment the offset returns to zero.
    ///
    /// Verified failable: routing `phase_offset` into `d.master` — the
    /// obvious "simplification" — breaks both halves.
    #[test]
    fn removing_an_offset_rejoins_the_untouched_trajectory() {
        const FREQ: u128 = 0x0123_4567;
        const OFFSET: u128 = 0xDEAD_BEEF;
        const N: usize = 40;

        // Reference: no offset, ever.
        let clean = run((0..N).map(|_| input(FREQ, 0)).collect());
        // Perturbed: offset applied for cycles 10..25, then removed.
        let perturbed = run((0..N)
            .map(|k| input(FREQ, if (10..25).contains(&k) { OFFSET } else { 0 }))
            .collect());

        let clean_master: Vec<u128> = clean.iter().map(|(_, m)| *m).collect();
        let perturbed_master: Vec<u128> = perturbed.iter().map(|(_, m)| *m).collect();
        assert_eq!(
            clean_master, perturbed_master,
            "the master trajectory must be untouched by an offset"
        );

        // Locate the divergence window rather than assuming where it
        // lands: `.with_reset(1)` shifts the sample indexing, and
        // hard-coding that shift makes the test fragile for no gain.
        let diff: Vec<usize> = (0..N).filter(|&k| perturbed[k].0 != clean[k].0).collect();

        assert!(
            !diff.is_empty(),
            "the offset must actually have changed the output — \
             otherwise this test passes because nothing happened"
        );
        let (first, last) = (diff[0], diff[diff.len() - 1]);

        // The window is exactly as long as the offset was applied (15
        // cycles), and contiguous: the offset takes effect and clears
        // cleanly, with no tail.
        assert_eq!(
            last - first + 1,
            15,
            "divergence should span exactly the 15 cycles the offset was applied"
        );
        assert_eq!(
            diff.len(),
            15,
            "the divergence window must be contiguous, not intermittent"
        );

        // And once the offset is gone the output has rejoined for good.
        assert!(last < N - 1, "the run must outlast the offset");
        for k in (last + 1)..N {
            assert_eq!(
                perturbed[k].0, clean[k].0,
                "output must stay rejoined to the untouched trajectory at cycle {k}"
            );
        }
    }

    /// Phase arithmetic wraps modulo `2^PHASE_W`; it must not saturate.
    #[test]
    fn phase_wraps_rather_than_saturating() {
        let max = (1u128 << W) - 1;
        let q = Q::<W> {
            master: bits::<W>(max),
        };
        let (o, d) =
            phase_accumulator_kernel::<W>(clock_reset(clock(true), reset(false)), input(2, 0), q);
        assert_eq!(d.master.raw(), 1, "accumulator wraps to 1, not saturates");
        assert_eq!(
            o.master.raw(),
            max,
            "the pre-wrap value is still observable"
        );

        let q = Q::<W> {
            master: bits::<W>(max),
        };
        let (o2, _d) =
            phase_accumulator_kernel::<W>(clock_reset(clock(true), reset(false)), input(0, 3), q);
        assert_eq!(o2.phase.raw(), 2, "output phase wraps too");
    }

    /// Hardware reset zeroes the accumulator. (A pulse or acquisition
    /// boundary must NOT do this — see the module docs.)
    #[test]
    fn hardware_reset_zeroes_the_accumulator() {
        let q = Q::<W> {
            master: bits::<W>(0xABCD),
        };
        let (o, d) =
            phase_accumulator_kernel::<W>(clock_reset(clock(true), reset(true)), input(7, 500), q);
        assert_eq!(d.master.raw(), 0);
        assert_eq!(o.phase.raw(), 0);
        assert_eq!(o.master.raw(), 0);
    }

    /// A frequency offset changes the slope; removing it does **not**
    /// erase the phase accumulated while it was active.
    ///
    /// This is physically correct and phase-continuous. Returning to the
    /// hypothetical unmodulated trajectory is a different operation
    /// needing a compensating phase correction — the two must not be
    /// confused.
    #[test]
    fn removing_a_frequency_offset_keeps_the_accumulated_phase() {
        const BASE: u128 = 1000;
        const EXTRA: u128 = 250;
        const N: usize = 30;

        let clean = run((0..N).map(|_| input(BASE, 0)).collect());
        let bumped = run((0..N)
            .map(|k| {
                input(
                    if (5..15).contains(&k) {
                        BASE + EXTRA
                    } else {
                        BASE
                    },
                    0,
                )
            })
            .collect());

        // After the offset is removed both advance at the same rate...
        let clean_step = clean[N - 1].1.wrapping_sub(clean[N - 2].1);
        let bumped_step = bumped[N - 1].1.wrapping_sub(bumped[N - 2].1);
        assert_eq!(clean_step, bumped_step, "slope returns to master");

        // ...but the accumulated lead persists.
        assert_ne!(
            clean[N - 1].1,
            bumped[N - 1].1,
            "phase accumulated during the offset must NOT be erased"
        );
    }
}
