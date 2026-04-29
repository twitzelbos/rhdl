//! PWM generator
//!
//! Pulse-width modulator with a saw-tooth counter and a single
//! comparator.  Period is fixed at `2^N` clock cycles; duty cycle
//! is supplied at runtime as a `Bits<N>` value.
//!
//! Output is high when `counter < duty`, low otherwise.  This gives
//! a duty cycle of `duty / 2^N`, ranging continuously from `0`
//! (always low) to `(2^N - 1) / 2^N` (almost always high).  An
//! exact 100% duty is not representable — gate the output
//! externally with the "always-on" condition if you need it.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+PwmGenerator+-----+
     |                        |
B<N> |                        | bool
+--->| duty             pwm   +--->
     |                        |
     +------------------------+
")]
//!
//!# Internals
//!
//! A single counter [DFF] of width `N`.  Each cycle the counter
//! increments (wrapping at `2^N`).  The combinational output is
//! `counter < duty`.  Period = `2^N` cycles, so for an `f_clk` of
//! 100 MHz and `N = 16`, the PWM frequency is
//! `100e6 / 65536 ≈ 1.5 kHz` (typical for LED dimming or motor
//! control).
//!
#![doc = badascii_doc::badascii!(r"
                +-+ DFF +-+
                |  count  |
                |       q +---+
        +------>|d        |   |
        |       +---------+   |
        |                     |   +--+CMP+--+
   +1   |                     +-->|q < duty +---> pwm
        |                         |         |
        +-------+1<------------+  +---------+
                                   ^
                                   |
              duty +----------------+
")]
//!
//!# Behavior
//!
//! - `duty = 0`: output is always `false`.
//! - `duty = (1 << (N-1))`: output is high for half the cycles
//!   (50% duty).
//! - `duty = (1 << N) - 1`: output is high for `2^N - 1` of every
//!   `2^N` cycles (the closest representable to 100%).
//! - The duty input is sampled combinationally — changing it
//!   mid-period takes effect on the *next* cycle's comparison.
//!   For glitch-free duty changes synchronized to the period
//!   boundary, register the duty externally.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/pwm.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/pwm.md")]
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// PWM generator core.
///
/// `N` is the bit width of both the period counter and the duty
/// input.  Period = `2^N` clock cycles.
pub struct PwmGenerator<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    counter: dff::DFF<Bits<N>>,
}

impl<const N: usize> SynchronousIO for PwmGenerator<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = Bits<N>;
    type O = bool;
    type Kernel = pwm<N>;
}

#[kernel]
/// Kernel for [PwmGenerator].
pub fn pwm<const N: usize>(cr: ClockReset, duty: Bits<N>, q: Q<N>) -> (bool, D<N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = D::<N>::dont_care();
    d.counter = q.counter + 1;
    let o = q.counter < duty;
    if cr.reset.any() {
        d.counter = bits(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_zero_duty_always_low() {
        let cr = ClockReset::dont_care();
        for c in 0u128..16 {
            let q = Q::<4> { counter: bits(c) };
            let (o, _d) = pwm::<4>(cr, bits(0), q);
            assert!(!o, "counter={c}, duty=0");
        }
    }

    #[test]
    fn test_max_duty_almost_always_high() {
        let cr = ClockReset::dont_care();
        // duty = 2^N - 1 = 15 for N=4: high when counter in [0, 15) = 15/16 cycles.
        for c in 0u128..16 {
            let q = Q::<4> { counter: bits(c) };
            let (o, _d) = pwm::<4>(cr, bits(15), q);
            assert_eq!(o, c < 15, "counter={c}, duty=15");
        }
    }

    #[test]
    fn test_half_duty_is_50_percent() {
        let cr = ClockReset::dont_care();
        // duty = 8 for N=4: high when counter in [0, 8) = 8/16 = 50%.
        let mut high = 0;
        for c in 0u128..16 {
            let q = Q::<4> { counter: bits(c) };
            let (o, _d) = pwm::<4>(cr, bits(8), q);
            if o {
                high += 1;
            }
        }
        assert_eq!(high, 8);
    }

    #[test]
    fn test_counter_increments_each_cycle() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> { counter: bits(7) };
        let (_o, d) = pwm::<4>(cr, bits(0), q);
        assert_eq!(d.counter, bits(8));
    }

    #[test]
    fn test_counter_wraps_at_max() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> { counter: bits(15) };
        let (_o, d) = pwm::<4>(cr, bits(0), q);
        assert_eq!(d.counter, bits(0));
    }

    #[test]
    fn test_reset_zeros_counter() {
        let cr = clock_reset(clock(true), reset(true));
        let q = Q::<4> { counter: bits(7) };
        let (_o, d) = pwm::<4>(cr, bits(8), q);
        assert_eq!(d.counter, bits(0));
    }

    // Tier 2 — iterator simulation: duty cycle measurement

    /// Run the PWM for one full period and verify the high count
    /// matches the duty value exactly.
    #[test]
    fn test_period_high_count_matches_duty() -> miette::Result<()> {
        for duty_val in [0u128, 1, 4, 8, 12, 15] {
            let uut = PwmGenerator::<4>::default();
            // Run for two full periods (32 cycles) so we get one
            // complete period after the reset transient.
            let stream = std::iter::repeat_n(bits::<4>(duty_val), 64)
                .with_reset(1)
                .clock_pos_edge(100);
            let outputs = uut
                .run(stream)
                .synchronous_sample()
                .filter(|s| !s.input.0.reset.any())
                .map(|s| s.output)
                .collect::<Vec<_>>();
            // Skip the first (partial) period to avoid reset-cycle aliasing.
            // Take the second full period of 16 samples.
            let second_period: Vec<bool> = outputs.iter().skip(16).take(16).copied().collect();
            let high = second_period.iter().filter(|x| **x).count();
            assert_eq!(
                high as u128, duty_val,
                "duty={duty_val}, second period {second_period:?}"
            );
        }
        Ok(())
    }

    // Tier 3 — HDL emission snapshot
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = PwmGenerator::<4>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [3:0] i, output wire [0:0] o);
               wire [4:0] od;
               wire [3:0] d;
               wire [3:0] q;
               assign o = od[0:0];
               top_counter c0(.clock_reset(clock_reset), .i(d[3:0]), .o(q[3:0]));
               assign d = od[4:1];
               assign od = kernel_pwm(clock_reset, i, q);
               function [4:0] kernel_pwm(input reg [1:0] arg_0, input reg [3:0] arg_1, input reg [3:0] arg_2);
                     reg [3:0] r0;
                     reg [3:0] r1;
                     // d
                     reg [3:0] r2;
                     reg [0:0] r3;
                     reg [3:0] r4;
                     reg [0:0] r5;
                     reg [1:0] r6;
                     reg [0:0] r7;
                     // d
                     reg [3:0] r8;
                     // d
                     reg [3:0] r9;
                     reg [4:0] r10;
                     localparam l0 = 4'b0001;
                     localparam l1 = 4'bXXXX;
                     localparam l2 = 4'b0000;
                     begin
                        r6 = arg_0;
                        r4 = arg_1;
                        r0 = arg_2;
                        r1 = r0 + l0;
                        r2 = l1;
                        r2[3:0] = r1;
                        r3 = r0 < r4;
                        r5 = r6[1:1];
                        r7 = |r5;
                        r8 = r2;
                        r8[3:0] = l2;
                        r9 = r7 ? r8 : r2;
                        r10 = {r9, r3};
                        kernel_pwm = r10;
                     end
               endfunction
            endmodule
            module top_counter(input wire [1:0] clock_reset, input wire [3:0] i, output reg [3:0] o);
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
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_pwm_hdl_works() -> miette::Result<()> {
        let uut = PwmGenerator::<4>::default();
        let stream = std::iter::repeat_n(bits::<4>(5), 32)
            .with_reset(1)
            .clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_pwm_trace() -> miette::Result<()> {
        let uut = PwmGenerator::<4>::default();
        // Sweep duty: 4, then 8, then 12 — three duty cycles in one trace.
        let mut pattern: Vec<Bits<4>> = Vec::new();
        for _ in 0..16 {
            pattern.push(bits(4));
        }
        for _ in 0..16 {
            pattern.push(bits(8));
        }
        for _ in 0..16 {
            pattern.push(bits(12));
        }
        let stream = pattern.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("pwm");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["541992df015b4f7bb6b2170d549135f6d7c32c476bf777e38fd0852996e4f2f1"];
        let digest = vcd.dump_to_file(root.join("pwm.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
