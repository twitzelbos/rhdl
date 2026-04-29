//! Strict-priority arbiter
//!
//! Arbitrates among `N` competing requesters by granting the
//! lowest-numbered active requester every cycle.  Compared to
//! [super::round_robin_arbiter::RoundRobinArbiter], this scheme is
//! trivial to implement (it's a thin wrapper over
//! [super::priority_encoder::priority_encoder_lsb]) but is *not* fair:
//! a high-priority requester that asks every cycle will starve every
//! lower-priority one.  Use this for fixed-priority interrupt
//! controllers, exception ranking, or as a baseline against which to
//! compare a fair arbiter under test.
//!
//! The I/O signature matches [super::round_robin_arbiter::RoundRobinArbiter]
//! exactly so the two are drop-in swappable in higher-level designs.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-+StrictPriorityArbiter+-+
     |                         |
B<N> |                         | Option<B<W>>
+--->| requests          grant +--->
     |                         |
     +-------------------------+
")]
//!
//!# Internals
//!
//! A single combinational call to [super::priority_encoder::priority_encoder_lsb].
//! No state.
//!
//!# Parameters
//!
//! - `N` — number of requesters
//! - `W` — bit width of the grant index, satisfying `2^W >= N`
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/strict_priority_arbiter.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/strict_priority_arbiter.md")]
use rhdl::prelude::*;

use super::priority_encoder::priority_encoder_lsb;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// Strict-priority arbiter core.
///
/// Stateless wrapper around [priority_encoder_lsb].  Same I/O as
/// [super::round_robin_arbiter::RoundRobinArbiter].
pub struct StrictPriorityArbiter<const N: usize, const W: usize>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth, {}

impl<const N: usize, const W: usize> SynchronousIO for StrictPriorityArbiter<N, W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    type I = Bits<N>;
    type O = Option<Bits<W>>;
    type Kernel = strict_priority_arbiter<N, W>;
}

#[kernel]
/// Kernel for [StrictPriorityArbiter].
pub fn strict_priority_arbiter<const N: usize, const W: usize>(
    _cr: ClockReset,
    requests: Bits<N>,
    _q: Q<N, W>,
) -> (Option<Bits<W>>, D<N, W>)
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let o = priority_encoder_lsb::<N, W>(requests);
    let d = D::<N, W>::dont_care();
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_no_requests_no_grant() {
        let cr = ClockReset::dont_care();
        let q = Q::<4, 2>::dont_care();
        let (o, _d) = strict_priority_arbiter::<4, 2>(cr, bits(0), q);
        assert_eq!(o, None);
    }

    #[test]
    fn test_lowest_request_wins_always() {
        let cr = ClockReset::dont_care();
        let q = Q::<4, 2>::dont_care();
        // 0b1010 -> bit 1 wins (bits 1 and 3 set, lowest is 1).
        assert_eq!(
            strict_priority_arbiter::<4, 2>(cr, bits(0b1010), q).0,
            Some(bits(1))
        );
        // 0b0011 -> bit 0 wins.
        assert_eq!(
            strict_priority_arbiter::<4, 2>(cr, bits(0b0011), q).0,
            Some(bits(0))
        );
        // 0b1000 -> bit 3 wins.
        assert_eq!(
            strict_priority_arbiter::<4, 2>(cr, bits(0b1000), q).0,
            Some(bits(3))
        );
        // 0b1111 -> bit 0 always wins.
        assert_eq!(
            strict_priority_arbiter::<4, 2>(cr, bits(0b1111), q).0,
            Some(bits(0))
        );
    }

    // Tier 2 — iterator simulation

    /// Demonstrates the starvation property: with bit 0 always asking
    /// alongside bit 2, bit 2 is never granted.
    #[test]
    fn test_starvation_under_persistent_priority() -> miette::Result<()> {
        let stream = std::iter::repeat_n(bits(0b0101), 16)
            .with_reset(1)
            .clock_pos_edge(100);
        let uut = StrictPriorityArbiter::<4, 2>::default();
        let grants = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        // Bit 0 always wins; bit 2 never gets a grant.
        assert!(grants.iter().all(|g| *g == Some(bits(0))), "{grants:?}");
        Ok(())
    }

    /// When the highest-priority requester goes idle, a lower one wins.
    #[test]
    fn test_idle_high_priority_lets_lower_win() -> miette::Result<()> {
        let inputs: Vec<Bits<4>> = vec![
            bits(0b0001), // bit 0 only
            bits(0b1110), // bits 1, 2, 3 (no bit 0)
            bits(0b1100), // bits 2, 3
            bits(0b1000), // bit 3 only
        ];
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let uut = StrictPriorityArbiter::<4, 2>::default();
        let grants = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        let expected = vec![Some(bits(0)), Some(bits(1)), Some(bits(2)), Some(bits(3))];
        assert_eq!(grants, expected);
        Ok(())
    }

    // Tier 3 — HDL emission snapshot
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = StrictPriorityArbiter::<4, 2>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [3:0] i, output wire [2:0] o);
               wire [2:0] od;
               assign o = od[2:0];
               assign od = kernel_strict_priority_arbiter(clock_reset, i);
               function [2:0] kernel_strict_priority_arbiter(input reg [1:0] arg_0, input reg [3:0] arg_1);
                     reg [3:0] r0;
                     reg [0:0] r1;
                     reg [0:0] r2;
                     // found
                     reg [0:0] r3;
                     // idx
                     reg [1:0] r4;
                     reg [3:0] r5;
                     reg [3:0] r6;
                     reg [0:0] r7;
                     reg [0:0] r8;
                     reg [0:0] r9;
                     // found
                     reg [0:0] r10;
                     // idx
                     reg [1:0] r11;
                     reg [3:0] r12;
                     reg [3:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     reg [0:0] r16;
                     // found
                     reg [0:0] r17;
                     // idx
                     reg [1:0] r18;
                     reg [3:0] r19;
                     reg [3:0] r20;
                     reg [0:0] r21;
                     reg [0:0] r22;
                     reg [0:0] r23;
                     // found
                     reg [0:0] r24;
                     // idx
                     reg [1:0] r25;
                     reg [2:0] r26;
                     reg [1:0] r27;
                     reg [2:0] r28;
                     reg [3:0] r29;
                     reg [1:0] r30;
                     reg [3:0] r31;
                     reg [4:0] r32;
                     reg [5:0] r33;
                     reg [6:0] r34;
                     localparam l0 = 4'b0001;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 2'b00;
                     localparam l5 = 2'b00;
                     localparam l6 = 4'b0001;
                     localparam l7 = 1'b1;
                     localparam l8 = 2'b01;
                     localparam l9 = 4'b0001;
                     localparam l10 = 1'b1;
                     localparam l11 = 2'b10;
                     localparam l12 = 4'b0001;
                     localparam l13 = 1'b1;
                     localparam l14 = 2'b11;
                     localparam l15 = 1'b1;
                     localparam l16 = 3'b000;
                     begin
                        r30 = arg_0;
                        r29 = arg_1;
                        r31 = r29[3:0];
                        r0 = r31 & l0;
                        r1 = |r0;
                        r2 = r1 & l1;
                        r3 = r2 ? l2 : l3;
                        r4 = r2 ? l4 : l5;
                        r32 = {{1{1'b0}}, r29};
                        r5 = r32[4:1];
                        r6 = r5 & l6;
                        r7 = |r6;
                        r8 = ~r3;
                        r9 = r7 & r8;
                        r10 = r9 ? l7 : r3;
                        r11 = r9 ? l8 : r4;
                        r33 = {{2{1'b0}}, r29};
                        r12 = r33[5:2];
                        r13 = r12 & l9;
                        r14 = |r13;
                        r15 = ~r10;
                        r16 = r14 & r15;
                        r17 = r16 ? l10 : r10;
                        r18 = r16 ? l11 : r11;
                        r34 = {{3{1'b0}}, r29};
                        r19 = r34[6:3];
                        r20 = r19 & l12;
                        r21 = |r20;
                        r22 = ~r17;
                        r23 = r21 & r22;
                        r24 = r23 ? l13 : r17;
                        r25 = r23 ? l14 : r18;
                        r27 = r25[1:0];
                        r26 = {l15, r27};
                        r28 = r24 ? r26 : l16;
                        kernel_strict_priority_arbiter = r28;
                     end
               endfunction
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_arbiter_hdl_works() -> miette::Result<()> {
        let uut = StrictPriorityArbiter::<4, 2>::default();
        let inputs: Vec<Bits<4>> = vec![
            bits(0b1111),
            bits(0b0101),
            bits(0b0000),
            bits(0b1000),
            bits(0b0010),
        ];
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_arbiter_trace() -> miette::Result<()> {
        let uut = StrictPriorityArbiter::<4, 2>::default();
        let inputs: Vec<Bits<4>> = vec![
            bits(0b0001),
            bits(0b1110),
            bits(0b0101),
            bits(0b1000),
            bits(0b0000),
            bits(0b1111),
        ];
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("strict_priority_arbiter");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["9411eb90f8e05776050936e6210329b058f0629da33b4fd4bb159dcce63d1fad"];
        let digest = vcd
            .dump_to_file(root.join("strict_priority_arbiter.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
