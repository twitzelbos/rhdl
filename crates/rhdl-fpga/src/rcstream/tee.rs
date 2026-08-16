#![warn(missing_docs)]
//! Split one [`RCStream`] of pairs into two independent streams.
//!
//! The exact inverse of [`super::zip`].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
     +--+RCStreamTee+---+
?(A,B)                  |  ?A
+---->| data     a_data +----->
      |          a_ready|<-----+
<-----+ ready           |  ?B
      |          b_data +----->
      |          b_ready|<-----+
      +-----------------+
")]
//!
//!# Naming: this splits, it does not duplicate
//!
//! `tee` here follows the convention already set by
//! [`crate::stream::tee`]: it takes a stream of **pairs** and separates
//! them, rather than duplicating one stream into two identical copies.
//! An input `Item<(A, B), (F, G)>` becomes an `Item<A, F>` on the `a`
//! output and an `Item<B, G>` on the `b` output — payloads and framing
//! markers split together, so neither side loses its framing.
//!
//! A genuine fan-out (send every item to *both* sinks) is a different
//! widget with a different hazard: the two sinks can become ready on
//! different cycles, so it needs per-branch "already delivered" state
//! to avoid handing the same item to one sink twice.  That is not this
//! widget, and it is deliberately not smuggled in here.
//!
//!# Lockstep production
//!
//! An input item is consumed only when **both** output buffers can
//! accept, and both halves are then emitted on the same cycle.  A sink
//! that stalls backpressures the whole widget rather than letting the
//! other side run ahead — which is what keeps the two outputs
//! index-aligned, so a downstream [`super::zip`] can recombine them.
//!
//!# Internals
//!
//! Three [`RCStreamRelay`] skid buffers: one on the input, one per
//! output.  No combinational path from any input to any output.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_tee.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_tee.md")]

use rhdl::prelude::*;

use super::bus::Item;
use super::relay::RCStreamRelay;

/// Split one [`RCStream`] of pairs into two independent streams.
///
/// `A`/`F` are the first output's payload and framing types, `B`/`G`
/// the second's.  The input is `RCStream<(A, B), (F, G)>`.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamTee<A: Digital, F: Digital, B: Digital, G: Digital> {
    /// Skid buffer for the paired input.
    input: RCStreamRelay<(A, B), (F, G)>,
    /// Skid buffer for the `a` output.
    a: RCStreamRelay<A, F>,
    /// Skid buffer for the `b` output.
    b: RCStreamRelay<B, G>,
}

impl<A: Digital, F: Digital, B: Digital, G: Digital> Default for RCStreamTee<A, F, B, G> {
    fn default() -> Self {
        Self {
            input: RCStreamRelay::default(),
            a: RCStreamRelay::default(),
            b: RCStreamRelay::default(),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`RCStreamTee`].
pub struct In<A: Digital, F: Digital, B: Digital, G: Digital> {
    /// Paired data flowing in from the upstream source.
    pub data: Option<Item<(A, B), (F, G)>>,
    /// Ready flowing in from the `a` sink.
    pub a_ready: bool,
    /// Ready flowing in from the `b` sink.
    pub b_ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [`RCStreamTee`].
pub struct Out<A: Digital, F: Digital, B: Digital, G: Digital> {
    /// Ready flowing out to the upstream source.
    pub ready: bool,
    /// Data flowing out to the `a` sink.
    pub a_data: Option<Item<A, F>>,
    /// Data flowing out to the `b` sink.
    pub b_data: Option<Item<B, G>>,
}

impl<A: Digital, F: Digital, B: Digital, G: Digital> SynchronousIO for RCStreamTee<A, F, B, G> {
    type I = In<A, F, B, G>;
    type O = Out<A, F, B, G>;
    type Kernel = tee_kernel<A, F, B, G>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
// The `(O, D)` return tuple is the framework-mandated kernel
// signature (CLAUDE.md §2); with four generic parameters it trips
// `clippy::type_complexity`.  Factoring it behind an alias would
// obscure the convention every other widget follows.
#[allow(clippy::type_complexity)]
pub fn tee_kernel<A: Digital, F: Digital, B: Digital, G: Digital>(
    _cr: ClockReset,
    i: In<A, F, B, G>,
    q: Q<A, F, B, G>,
) -> (Out<A, F, B, G>, D<A, F, B, G>) {
    let mut d = D::<A, F, B, G>::dont_care();

    // Feed the input skid buffer.
    d.input.data = i.data;

    // Split only when we have an item AND both output buffers can take
    // their half; then consume the input and emit both halves together.
    let (have, item) = match q.input.data {
        Some(it) => (true, it),
        None => (false, Item::<(A, B), (F, G)>::dont_care()),
    };
    let fire = have && q.a.ready && q.b.ready;

    d.input.ready = fire;
    d.a.data = if fire {
        Some(Item::<A, F> {
            data: item.data.0,
            frame: item.frame.0,
        })
    } else {
        None
    };
    d.b.data = if fire {
        Some(Item::<B, G> {
            data: item.data.1,
            frame: item.frame.1,
        })
    } else {
        None
    };

    // Downstream readies feed the respective output buffers.
    d.a.ready = i.a_ready;
    d.b.ready = i.b_ready;

    let o = Out::<A, F, B, G> {
        ready: q.input.ready,
        a_data: q.a.data,
        b_data: q.b.data,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcstream::bus::RCStream;
    use rhdl::core::sim::ResetOrData;

    type Tee = RCStreamTee<b8, bool, b4, ()>;

    fn paired(a: u128, af: bool, b: u128) -> Item<(b8, b4), (bool, ())> {
        Item::<(b8, b4), (bool, ())> {
            data: (bits::<8>(a), bits::<4>(b)),
            frame: (af, ()),
        }
    }

    fn q_with(
        data: Option<Item<(b8, b4), (bool, ())>>,
        a_ready: bool,
        b_ready: bool,
    ) -> Q<b8, bool, b4, ()> {
        Q::<b8, bool, b4, ()> {
            input: RCStream::<(b8, b4), (bool, ())> { data, ready: true },
            a: RCStream::<b8, bool> {
                data: None,
                ready: a_ready,
            },
            b: RCStream::<b4, ()> {
                data: None,
                ready: b_ready,
            },
        }
    }

    fn idle_in() -> In<b8, bool, b4, ()> {
        In::<b8, bool, b4, ()> {
            data: None,
            a_ready: true,
            b_ready: true,
        }
    }

    /// Tier 1 — a paired item splits into both halves, each keeping its
    /// own framing marker.
    #[test]
    fn splits_pair_into_both_outputs() {
        let q = q_with(Some(paired(0xAB, true, 0x5)), true, true);
        let (_o, d) = tee_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
        match d.a.data {
            Some(it) => {
                assert_eq!(it.data.raw(), 0xAB);
                assert!(
                    it.frame,
                    "the a-side framing marker must follow the a payload"
                );
            }
            None => panic!("expected an `a` half"),
        }
        match d.b.data {
            Some(it) => assert_eq!(it.data.raw(), 0x5),
            None => panic!("expected a `b` half"),
        }
        assert!(d.input.ready, "the input is consumed when both halves go");
    }

    /// Tier 1 — if either output buffer is full, nothing moves.  Emitting
    /// only one half would desynchronise the two streams permanently.
    #[test]
    fn one_full_output_stalls_the_split() {
        for (a_ready, b_ready) in [(true, false), (false, true), (false, false)] {
            let q = q_with(Some(paired(0xAB, false, 0x5)), a_ready, b_ready);
            let (_o, d) = tee_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
            assert!(
                d.a.data.is_none() && d.b.data.is_none(),
                "neither half may be emitted unless both can be (a_ready={a_ready}, b_ready={b_ready})"
            );
            assert!(
                !d.input.ready,
                "the input must be held while either output is blocked"
            );
        }
    }

    /// Tier 1 — an empty input emits nothing and consumes nothing.
    #[test]
    fn empty_input_is_inert() {
        let q = q_with(None, true, true);
        let (_o, d) = tee_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
        assert!(d.a.data.is_none() && d.b.data.is_none());
        assert!(!d.input.ready, "nothing buffered means nothing to consume");
    }

    /// Tier 1 — the per-sink readies reach their own output buffers, and
    /// the upstream ready comes from the input buffer.
    #[test]
    fn handshakes_are_wired_to_their_own_buffers() {
        let mut q = q_with(None, true, true);
        q.input.ready = true;
        let i = In::<b8, bool, b4, ()> {
            data: None,
            a_ready: true,
            b_ready: false,
        };
        let (o, d) = tee_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), i, q);
        assert!(d.a.ready, "a's sink ready must reach a's buffer");
        assert!(!d.b.ready, "b's sink ready must reach b's buffer");
        assert!(o.ready, "upstream ready comes from the input buffer");
    }

    /// The LID requirement: no combinational path from any input to any
    /// output.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Tee::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed-loop end-to-end with the two sinks draining at
    /// *different* rates.  Both outputs must still receive every half in
    /// order: the slower sink backpressures the whole widget rather than
    /// letting the faster one run ahead, which is what keeps the two
    /// outputs index-aligned.
    #[test]
    fn both_outputs_stay_aligned_despite_mismatched_sink_rates() -> Result<(), RHDLError> {
        const COUNT: u128 = 24;
        let uut = Tee::default();
        let mut sent: u128 = 0;
        let mut got_a: Vec<(u128, bool)> = Vec::new();
        let mut got_b: Vec<u128> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                // `a` drains most cycles; `b` only every third.
                let a_ready = !phase.is_multiple_of(5);
                let b_ready = phase.is_multiple_of(3);
                if let Some(it) = output.a_data {
                    if a_ready {
                        got_a.push((it.data.raw(), it.frame));
                    }
                }
                if let Some(it) = output.b_data {
                    if b_ready {
                        got_b.push(it.data.raw());
                    }
                }
                let mut input = In::<b8, bool, b4, ()> {
                    data: None,
                    a_ready,
                    b_ready,
                };
                if sent < COUNT && output.ready {
                    input.data = Some(paired(sent, sent % 4 == 3, sent & 0xF));
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 200_000)
        .for_each(drop);

        let expect_a: Vec<(u128, bool)> = (0..COUNT).map(|k| (k, k % 4 == 3)).collect();
        let expect_b: Vec<u128> = (0..COUNT).map(|k| k & 0xF).collect();
        assert_eq!(
            got_a, expect_a,
            "the `a` half must arrive complete and in order"
        );
        assert_eq!(
            got_b, expect_b,
            "the `b` half must arrive complete and in order"
        );
        Ok(())
    }

    /// Build the open-loop stimulus for the codegen tiers.
    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, bool, b4, ()>)>> {
        (0..24u128)
            .map(|k| In::<b8, bool, b4, ()> {
                data: Some(paired(k, k % 4 == 3, k & 0xF)),
                a_ready: k % 3 != 0,
                b_ready: k % 2 == 0,
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot of the widget's own module.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Tee::default();
        let desc = uut.descriptor("rcstream_tee".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_tee")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_tee(input wire [1:0] clock_reset, input wire [15:0] i, output wire [15:0] o);
               wire [47:0] od;
               wire [31:0] d;
               wire [31:0] q;
               assign o = od[15:0];
               rcstream_tee_input c0(.clock_reset(clock_reset), .i(d[14:0]), .o(q[14:0]));
               rcstream_tee_a c1(.clock_reset(clock_reset), .i(d[25:15]), .o(q[25:15]));
               rcstream_tee_b c2(.clock_reset(clock_reset), .i(d[31:26]), .o(q[31:26]));
               assign d = od[47:16];
               assign od = kernel_tee_kernel(clock_reset, i, q);
               function [47:0] kernel_tee_kernel(input reg [1:0] arg_0, input reg [15:0] arg_1, input reg [31:0] arg_2);
                     reg [13:0] r0;
                     reg [15:0] r1;
                     // d
                     reg [31:0] r2;
                     reg [14:0] r3;
                     reg [31:0] r4;
                     reg [13:0] r5;
                     reg [0:0] r6;
                     reg [12:0] r7;
                     reg [13:0] r8;
                     reg [13:0] r9;
                     reg [0:0] r10;
                     reg [12:0] r11;
                     reg [10:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [5:0] r15;
                     reg [0:0] r16;
                     reg [0:0] r17;
                     // d
                     reg [31:0] r18;
                     reg [11:0] r19;
                     reg [7:0] r20;
                     reg [0:0] r21;
                     reg [8:0] r22;
                     reg [8:0] r23;
                     reg [9:0] r24;
                     reg [8:0] r25;
                     reg [9:0] r26;
                     // d
                     reg [31:0] r27;
                     reg [11:0] r28;
                     reg [3:0] r29;
                     reg [3:0] r30;
                     reg [4:0] r31;
                     reg [3:0] r32;
                     reg [4:0] r33;
                     // d
                     reg [31:0] r34;
                     reg [0:0] r35;
                     // d
                     reg [31:0] r36;
                     reg [0:0] r37;
                     // d
                     reg [31:0] r38;
                     reg [14:0] r39;
                     reg [0:0] r40;
                     reg [10:0] r41;
                     reg [9:0] r42;
                     reg [5:0] r43;
                     reg [4:0] r44;
                     reg [15:0] r45;
                     reg [15:0] r46;
                     reg [15:0] r47;
                     reg [47:0] r48;
                     reg [1:0] r49;
                     localparam l0 = 32'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 14'bXXXXXXXXXXXXX0;
                     localparam l5 = 9'b000000000;
                     localparam l6 = 1'b1;
                     localparam l7 = 10'b0000000000;
                     localparam l8 = 4'b0000;
                     localparam l9 = 1'b1;
                     localparam l10 = 5'b00000;
                     localparam l11 = 16'b0000000000000000;
                     begin
                        r49 = arg_0;
                        r1 = arg_1;
                        r4 = arg_2;
                        r0 = r1[13:0];
                        r2 = l0;
                        r2[13:0] = r0;
                        r3 = r4[14:0];
                        r5 = r3[13:0];
                        r6 = r5[13:13];
                        r7 = r5[12:0];
                        r8 = {r7, l1};
                        case (r6)
                           1'b1 : r9 = r8;
                           1'b0 : r9 = l4;
                        endcase
                        r10 = r9[0:0];
                        r11 = r9[13:1];
                        r12 = r4[25:15];
                        r13 = r12[10:10];
                        r14 = r10 & r13;
                        r15 = r4[31:26];
                        r16 = r15[5:5];
                        r17 = r14 & r16;
                        r18 = r2;
                        r18[14:14] = r17;
                        r19 = r11[11:0];
                        r20 = r19[7:0];
                        r21 = r11[12:12];
                        r22 = l5;
                        r22[7:0] = r20;
                        r23 = r22;
                        r23[8:8] = r21;
                        r25 = r23[8:0];
                        r24 = {l6, r25};
                        r26 = r17 ? r24 : l7;
                        r27 = r18;
                        r27[24:15] = r26;
                        r28 = r11[11:0];
                        r29 = r28[11:8];
                        r30 = l8;
                        r30[3:0] = r29;
                        r32 = r30[3:0];
                        r31 = {l9, r32};
                        r33 = r17 ? r31 : l10;
                        r34 = r27;
                        r34[30:26] = r33;
                        r35 = r1[14:14];
                        r36 = r34;
                        r36[25:25] = r35;
                        r37 = r1[15:15];
                        r38 = r36;
                        r38[31:31] = r37;
                        r39 = r4[14:0];
                        r40 = r39[14:14];
                        r41 = r4[25:15];
                        r42 = r41[9:0];
                        r43 = r4[31:26];
                        r44 = r43[4:0];
                        r45 = l11;
                        r45[0:0] = r40;
                        r46 = r45;
                        r46[10:1] = r42;
                        r47 = r46;
                        r47[15:11] = r44;
                        r48 = {r38, r47};
                        kernel_tee_kernel = r48;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip on both RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = Tee::default();
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = Tee::default();
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_tee");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "b8db45ab710f956838d5b0cdcd9eb85189a83823a5ef8ba91ab98ad8b50f1d1c"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_tee.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
