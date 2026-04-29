//! Pulse stretcher / one-shot
//!
//! Stretches a single-cycle pulse on the input into a multi-cycle held
//! pulse on the output.  Whenever the input is `true`, an internal
//! counter is loaded with the configured `stretch` value.  While the
//! counter is non-zero, the output stays `true`; on every cycle that
//! the input is `false`, the counter decrements by one.  This is the
//! level-retriggerable variant: a high input always re-arms the timer,
//! which is the form most useful as a building block for debouncers,
//! watchdogs, blink-on-event indicators, and short-pulse capture.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--+PulseStretcher+--+
     |                    |
bool |                    | bool
+--->| input       output +--->
     |                    |
     +--------------------+
")]
//!
//!# Internals
//!
//! Internally, the stretcher composes a counter [DFF] with a constant
//! holding the configured stretch length.
//!
#![doc = badascii_doc::badascii!(r"
                +--+Constant+
                |    stretch|
                +-----+-----+
                      |
                      v
                +-+MUX+--+
input +-------->|i==1    |    +-+DFF+-+
                |        +--->|d    q+-+--->(counter != 0) = output
                | else   |    |      | |
        +------>|q-1     |    +------+ |
        |       +--------+             |
        +------------------------------+
")]
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/pulse_stretcher.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/pulse_stretcher.md")]
use rhdl::prelude::*;

use super::{constant::Constant, dff};

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// Pulse stretcher core.
///
/// `N` is the bit width of the internal counter, so the maximum
/// stretch length is `2^N - 1` cycles.  The actual stretch value
/// is provided at construction time via [Self::new].
pub struct PulseStretcher<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    counter: dff::DFF<Bits<N>>,
    stretch: Constant<Bits<N>>,
}

impl<const N: usize> PulseStretcher<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    /// Create a new pulse stretcher that holds its output high for
    /// `stretch_cycles` clocks after each high input sample.
    ///
    /// A `stretch_cycles` value of zero produces a degenerate
    /// stretcher whose output is always low.
    pub fn new(stretch_cycles: Bits<N>) -> Self {
        Self {
            counter: dff::DFF::default(),
            stretch: Constant::new(stretch_cycles),
        }
    }
}

impl<const N: usize> SynchronousIO for PulseStretcher<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = bool;
    type O = bool;
    type Kernel = pulse_stretcher<N>;
}

#[kernel]
/// Kernel for [PulseStretcher].
pub fn pulse_stretcher<const N: usize>(cr: ClockReset, i: bool, q: Q<N>) -> (bool, D<N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let next_count = if i {
        q.stretch
    } else if q.counter != bits(0) {
        q.counter - 1
    } else {
        bits(0)
    };
    let mut d = D::<N>::dont_care();
    d.counter = next_count;
    let o = q.counter != bits(0);
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
    fn test_idle_stays_idle() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> {
            counter: bits(0),
            stretch: bits(5),
        };
        let (o, d) = pulse_stretcher::<4>(cr, false, q);
        assert!(!o);
        assert_eq!(d.counter, bits(0));
    }

    #[test]
    fn test_input_high_arms_counter() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> {
            counter: bits(0),
            stretch: bits(5),
        };
        let (o, d) = pulse_stretcher::<4>(cr, true, q);
        // Output reflects current (still-zero) counter.
        assert!(!o);
        // Next-cycle counter is loaded with the stretch length.
        assert_eq!(d.counter, bits(5));
    }

    #[test]
    fn test_armed_counter_decrements_when_input_low() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> {
            counter: bits(3),
            stretch: bits(5),
        };
        let (o, d) = pulse_stretcher::<4>(cr, false, q);
        assert!(o);
        assert_eq!(d.counter, bits(2));
    }

    #[test]
    fn test_input_high_retriggers_mid_stretch() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> {
            counter: bits(2),
            stretch: bits(5),
        };
        let (o, d) = pulse_stretcher::<4>(cr, true, q);
        assert!(o);
        // Re-arm should fully reload, not decrement.
        assert_eq!(d.counter, bits(5));
    }

    #[test]
    fn test_counter_at_one_returns_to_zero() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> {
            counter: bits(1),
            stretch: bits(5),
        };
        let (o, d) = pulse_stretcher::<4>(cr, false, q);
        assert!(o);
        assert_eq!(d.counter, bits(0));
    }

    #[test]
    fn test_reset_clears_counter() {
        let cr = clock_reset(clock(true), reset(true));
        // Even with a high input that would re-arm, reset must zero the next counter.
        let q = Q::<4> {
            counter: bits(7),
            stretch: bits(5),
        };
        let (_o, d) = pulse_stretcher::<4>(cr, true, q);
        assert_eq!(d.counter, bits(0));
    }

    // Tier 2 — iterator-based simulation tests

    /// A short input pulse (1 cycle) followed by idle should produce
    /// a `STRETCH`-cycle output pulse.
    #[test]
    fn test_single_pulse_stretches_to_full_length() -> miette::Result<()> {
        const STRETCH: u128 = 5;
        let mut input = vec![false; 20];
        input[2] = true;
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let uut = PulseStretcher::<4>::new(bits(STRETCH));
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        let high_run = outputs.iter().filter(|x| **x).count();
        assert_eq!(high_run as u128, STRETCH);
        Ok(())
    }

    /// Re-triggering during the stretch window should extend it.
    #[test]
    fn test_retrigger_extends_stretch() -> miette::Result<()> {
        const STRETCH: u128 = 4;
        // Pulses at idx 1 and idx 3 — second should fully re-arm,
        // so total high run = (3 - 1) + STRETCH = 2 + 4 = 6.
        let mut input = vec![false; 20];
        input[1] = true;
        input[3] = true;
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let uut = PulseStretcher::<4>::new(bits(STRETCH));
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        let high_run = outputs.iter().filter(|x| **x).count();
        assert_eq!(high_run as u128, (3 - 1) + STRETCH);
        Ok(())
    }

    // Tier 3 — HDL emission snapshot

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = PulseStretcher::<4>::new(bits(5));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [0:0] i, output wire [0:0] o);
               wire [4:0] od;
               wire [3:0] d;
               wire [7:0] q;
               assign o = od[0:0];
               top_counter c0(.clock_reset(clock_reset), .i(d[3:0]), .o(q[3:0]));
               top_stretch c1(.clock_reset(clock_reset), .o(q[7:4]));
               assign d = od[4:1];
               assign od = kernel_pulse_stretcher(clock_reset, i, q);
               function [4:0] kernel_pulse_stretcher(input reg [1:0] arg_0, input reg [0:0] arg_1, input reg [7:0] arg_2);
                     reg [3:0] r0;
                     reg [7:0] r1;
                     reg [3:0] r2;
                     reg [0:0] r3;
                     reg [3:0] r4;
                     reg [3:0] r5;
                     reg [3:0] r6;
                     reg [3:0] r7;
                     reg [0:0] r8;
                     // d
                     reg [3:0] r9;
                     reg [3:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     reg [1:0] r13;
                     reg [0:0] r14;
                     // d
                     reg [3:0] r15;
                     // d
                     reg [3:0] r16;
                     reg [4:0] r17;
                     localparam l0 = 4'b0001;
                     localparam l1 = 4'b0000;
                     localparam l2 = 4'bXXXX;
                     localparam l3 = 4'b0000;
                     begin
                        r13 = arg_0;
                        r8 = arg_1;
                        r1 = arg_2;
                        r0 = r1[7:4];
                        r2 = r1[3:0];
                        r3 = |r2;
                        r4 = r1[3:0];
                        r5 = r4 - l0;
                        r6 = r3 ? r5 : l1;
                        r7 = r8 ? r0 : r6;
                        r9 = l2;
                        r9[3:0] = r7;
                        r10 = r1[3:0];
                        r11 = |r10;
                        r12 = r13[1:1];
                        r14 = |r12;
                        r15 = r9;
                        r15[3:0] = l3;
                        r16 = r14 ? r15 : r9;
                        r17 = {r16, r11};
                        kernel_pulse_stretcher = r17;
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
            module top_stretch(input wire [1:0] clock_reset, output wire [3:0] o);
               assign o = 4'b0101;
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // Tier 4 — iverilog round-trip (RTL and NTL)

    #[test]
    fn test_pulse_stretcher_hdl_works() -> miette::Result<()> {
        let uut = PulseStretcher::<4>::new(bits(5));
        let mut input = vec![false; 20];
        input[2] = true;
        input[10] = true;
        input[12] = true;
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest

    #[test]
    fn test_pulse_stretcher_trace() -> miette::Result<()> {
        let uut = PulseStretcher::<4>::new(bits(5));
        let mut input = vec![false; 20];
        input[2] = true;
        input[10] = true;
        input[12] = true;
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("pulse_stretcher");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["cc9a1c32e18b963f94ae8e85eb2a402d36c561e5ef0a08b9b773f7f956d207c4"];
        let digest = vcd.dump_to_file(root.join("pulse_stretcher.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
