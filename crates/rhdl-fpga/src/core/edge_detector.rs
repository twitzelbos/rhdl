//! Edge detector
//!
//! Detects rising, falling, and any edges on a single-bit input.
//! On every clock cycle, the current sample is compared against the
//! previous sample; the three output flags assert for exactly one cycle
//! when a transition is observed.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +----+EdgeDetector+----+
     |                      |
     |               rising +--->
bool |                      |
+--->| input        falling +--->
     |                      |
     |                  any +--->
     +----------------------+
")]
//!
//!# Internals
//!
//! Internally, the edge detector is a single [DFF] that holds the
//! previous sample of the input.  The output flags are pure
//! combinational functions of the current input and the held sample.
//!
#![doc = badascii_doc::badascii!(r"

input +----------+--------------------------+rising
                 |                +-+AND+-+ +------>
                 |          +---->+       |
                 |          |     +-------+
                 v          |
                +-+    +-+  |
                |D|    |Q+--+   +-+NOT+-+
                | |    | +----->+       +--+
                | |    | |      +-------+  |
                +-+    +-+                 |
                                           v
input +-----+-+NOT+-+--------------------+-+-+falling
            |       |             +-+AND+-+ +----->
            +-------+        +---->+       |
                             |     +-------+
                             |
                             |
                             |     +-+OR+--+ any
                             +---->+       +------>
                                   +-------+
")]
//!
//!# Reset semantics
//!
//! The held sample resets to `false`.  During reset, all three output
//! flags are forced low so no edge is reported until the cycle after
//! reset is released.  After release, an input that is `true` on the
//! first non-reset cycle will register as a rising edge, since the
//! prior sample is the post-reset value.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/edge_detector.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/edge_detector.md")]
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// Edge detector core.
///
/// Holds the previous sample of the input in a single
/// flip flop and reports rising / falling / any edges
/// on the next cycle.
pub struct EdgeDetector {
    prev: dff::DFF<bool>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [EdgeDetector].
pub struct Edges {
    /// Asserted for one cycle when the input transitions from `false` to `true`.
    pub rising: bool,
    /// Asserted for one cycle when the input transitions from `true` to `false`.
    pub falling: bool,
    /// Asserted for one cycle on either a rising or falling transition.
    pub any: bool,
}

impl SynchronousIO for EdgeDetector {
    type I = bool;
    type O = Edges;
    type Kernel = edge_detector;
}

#[kernel]
/// Kernel for [EdgeDetector].
pub fn edge_detector(cr: ClockReset, i: bool, q: Q) -> (Edges, D) {
    let rising = i && !q.prev;
    let falling = !i && q.prev;
    let any = rising || falling;
    let mut d = D::dont_care();
    d.prev = i;
    let mut o = Edges::dont_care();
    o.rising = rising;
    o.falling = falling;
    o.any = any;
    if cr.reset.any() {
        d.prev = false;
        o.rising = false;
        o.falling = false;
        o.any = false;
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
    fn test_no_edge_when_low_held() {
        let cr = ClockReset::dont_care();
        let q = Q { prev: false };
        let (o, d) = edge_detector(cr, false, q);
        assert!(!o.rising);
        assert!(!o.falling);
        assert!(!o.any);
        assert!(!d.prev);
    }

    #[test]
    fn test_no_edge_when_high_held() {
        let cr = ClockReset::dont_care();
        let q = Q { prev: true };
        let (o, d) = edge_detector(cr, true, q);
        assert!(!o.rising);
        assert!(!o.falling);
        assert!(!o.any);
        assert!(d.prev);
    }

    #[test]
    fn test_rising_edge() {
        let cr = ClockReset::dont_care();
        let q = Q { prev: false };
        let (o, d) = edge_detector(cr, true, q);
        assert!(o.rising);
        assert!(!o.falling);
        assert!(o.any);
        assert!(d.prev);
    }

    #[test]
    fn test_falling_edge() {
        let cr = ClockReset::dont_care();
        let q = Q { prev: true };
        let (o, d) = edge_detector(cr, false, q);
        assert!(!o.rising);
        assert!(o.falling);
        assert!(o.any);
        assert!(!d.prev);
    }

    #[test]
    fn test_reset_forces_outputs_low_and_clears_prev() {
        // Even if input is true and prev is false (a would-be rising edge),
        // the reset path must suppress all edge flags and reset prev to false.
        let cr = clock_reset(clock(true), reset(true));
        let q = Q { prev: true };
        let (o, d) = edge_detector(cr, true, q);
        assert!(!o.rising);
        assert!(!o.falling);
        assert!(!o.any);
        assert!(!d.prev);
    }

    // Tier 2 — iterator-based simulation tests

    fn fixed_pattern() -> Vec<bool> {
        // 0,0,1,1,0,0,1,0,0,1,1,1,0,0
        // Expected edges at samples (relative to first non-reset cycle):
        //   rising:  idx 2, 6, 9
        //   falling: idx 4, 7, 12
        vec![
            false, false, true, true, false, false, true, false, false, true, true, true, false,
            false,
        ]
    }

    #[test]
    fn test_edge_detector_stream() -> miette::Result<()> {
        let inputs = fixed_pattern();
        let stream = inputs.iter().copied().with_reset(1).clock_pos_edge(100);
        let uut = EdgeDetector::default();
        let outputs = uut.run(stream).synchronous_sample().collect::<Vec<_>>();
        // Drop the reset cycle and pair each (input_n, output_n).  The output
        // for cycle n compares input_n against the sample latched at cycle n-1.
        let post_reset = outputs
            .iter()
            .filter(|s| !s.input.0.reset.any())
            .collect::<Vec<_>>();
        // Expected per-cycle outputs for the fixed_pattern input above.
        let expected = [
            // (rising, falling, any)
            (false, false, false), // 0: prev=0 -> in=0
            (false, false, false), // 0: prev=0 -> in=0
            (true, false, true),   // 1: prev=0 -> in=1
            (false, false, false), // 1: prev=1 -> in=1
            (false, true, true),   // 0: prev=1 -> in=0
            (false, false, false), // 0: prev=0 -> in=0
            (true, false, true),   // 1: prev=0 -> in=1
            (false, true, true),   // 0: prev=1 -> in=0
            (false, false, false), // 0: prev=0 -> in=0
            (true, false, true),   // 1: prev=0 -> in=1
            (false, false, false), // 1: prev=1 -> in=1
            (false, false, false), // 1: prev=1 -> in=1
            (false, true, true),   // 0: prev=1 -> in=0
            (false, false, false), // 0: prev=0 -> in=0
        ];
        assert_eq!(post_reset.len(), expected.len());
        for (i, (sample, exp)) in post_reset.iter().zip(expected.iter()).enumerate() {
            let o = sample.output;
            assert_eq!(
                (o.rising, o.falling, o.any),
                *exp,
                "mismatch at non-reset cycle {i}"
            );
        }
        Ok(())
    }

    // Tier 3 — HDL emission snapshot

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = EdgeDetector::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [0:0] i, output wire [2:0] o);
               wire [3:0] od;
               wire [0:0] d;
               wire [0:0] q;
               assign o = od[2:0];
               top_prev c0(.clock_reset(clock_reset), .i(d[0:0]), .o(q[0:0]));
               assign d = od[3:3];
               assign od = kernel_edge_detector(clock_reset, i, q);
               function [3:0] kernel_edge_detector(input reg [1:0] arg_0, input reg [0:0] arg_1, input reg [0:0] arg_2);
                     reg [0:0] r0;
                     reg [0:0] r1;
                     reg [0:0] r2;
                     reg [0:0] r3;
                     reg [0:0] r4;
                     reg [0:0] r5;
                     reg [0:0] r6;
                     // d
                     reg [0:0] r7;
                     // o
                     reg [2:0] r8;
                     // o
                     reg [2:0] r9;
                     // o
                     reg [2:0] r10;
                     reg [0:0] r11;
                     reg [1:0] r12;
                     reg [0:0] r13;
                     // d
                     reg [0:0] r14;
                     // o
                     reg [2:0] r15;
                     // o
                     reg [2:0] r16;
                     // o
                     reg [2:0] r17;
                     // d
                     reg [0:0] r18;
                     // o
                     reg [2:0] r19;
                     reg [3:0] r20;
                     localparam l0 = 1'bX;
                     localparam l1 = 3'bXXX;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b0;
                     localparam l4 = 1'b0;
                     localparam l5 = 1'b0;
                     begin
                        r12 = arg_0;
                        r3 = arg_1;
                        r0 = arg_2;
                        r1 = ~r0;
                        r2 = r3 & r1;
                        r4 = ~r3;
                        r5 = r4 & r0;
                        r6 = r2 | r5;
                        r7 = l0;
                        r7[0:0] = r3;
                        r8 = l1;
                        r8[0:0] = r2;
                        r9 = r8;
                        r9[1:1] = r5;
                        r10 = r9;
                        r10[2:2] = r6;
                        r11 = r12[1:1];
                        r13 = |r11;
                        r14 = r7;
                        r14[0:0] = l2;
                        r15 = r10;
                        r15[0:0] = l3;
                        r16 = r15;
                        r16[1:1] = l4;
                        r17 = r16;
                        r17[2:2] = l5;
                        r18 = r13 ? r14 : r7;
                        r19 = r13 ? r17 : r10;
                        r20 = {r18, r19};
                        kernel_edge_detector = r20;
                     end
               endfunction
            endmodule
            module top_prev(input wire [1:0] clock_reset, input wire [0:0] i, output reg [0:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 1'b0;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 1'b0;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // Tier 4 — iverilog round-trip (RTL and NTL)

    #[test]
    fn test_edge_detector_hdl_works() -> miette::Result<()> {
        let uut = EdgeDetector::default();
        let inputs = fixed_pattern();
        let stream = inputs.iter().copied().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest

    #[test]
    fn test_edge_detector_trace() -> miette::Result<()> {
        let uut = EdgeDetector::default();
        let inputs = fixed_pattern();
        let stream = inputs.iter().copied().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("edge_detector");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["061d60bdeba3ec0557650a069f8cf1c10bf9d10f1fe7c1cb6bb350a26eb8d480"];
        let digest = vcd.dump_to_file(root.join("edge_detector.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
