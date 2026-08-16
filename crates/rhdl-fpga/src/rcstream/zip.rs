#![warn(missing_docs)]
//! Combine two [`RCStream`]s into one stream of pairs.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
       +-+RCStreamZip+---+
 ?A    |                 |  ?(A,B)
+----->| a_data     data +------->
<------+ a_ready         |
 ?B    |                 |
+----->| b_data    ready |<------+
<------+ b_ready         |
       +-----------------+
")]
//!
//!# Framing: both markers are carried, neither is chosen
//!
//! The output is `RCStream<(A, B), (F, G)>` — the payloads pair up and
//! **so do the framing markers**.
//!
//! The obvious alternative was to require `F == G` and emit one of
//! them.  That was rejected: the two markers are independent run-time
//! values, so picking one silently discards the other, and there is no
//! principled basis for preferring the `a` side.  Two streams that are
//! zipped are not thereby synchronised in their framing — `a` may end a
//! frame on an item where `b` does not.  Carrying `(F, G)` keeps that
//! observable; a consumer that genuinely wants a single marker can
//! `map` the pair down to one with an explicit rule it chose itself.
//!
//! If neither side is framed, `F = G = ()` and `(F, G)` costs no wire
//! bits.
//!
//!# Lockstep consumption
//!
//! An output pair is produced only when **both** inputs have an item
//! *and* the output buffer can accept it.  When that holds, both inputs
//! are consumed on the same cycle.  A stream that runs ahead simply
//! backpressures until its partner catches up — the widget never
//! buffers one side unboundedly.
//!
//!# Internals
//!
//! Three [`RCStreamRelay`] skid buffers: one per input, one on the
//! output.  No combinational path from any input to any output.
//!
#![doc = badascii_doc::badascii!("
   a  +-------+
  --->| Relay +--+
      +-------+  |   +-------+  +-------+
                 +-->| pair  +->| Relay +--->
   b  +-------+  |   +-------+  +-------+
  --->| Relay +--+     fires when both
      +-------+        present & out ready
")]
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_zip.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_zip.md")]

use rhdl::prelude::*;

use super::bus::Item;
use super::relay::RCStreamRelay;

/// Combine two [`RCStream`]s into one stream of pairs.
///
/// `A`/`F` are the first stream's payload and framing types, `B`/`G`
/// the second's.  The output is `RCStream<(A, B), (F, G)>`.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamZip<A: Digital, F: Digital, B: Digital, G: Digital> {
    /// Skid buffer for the `a` input.
    a: RCStreamRelay<A, F>,
    /// Skid buffer for the `b` input.
    b: RCStreamRelay<B, G>,
    /// Skid buffer for the paired output.
    out: RCStreamRelay<(A, B), (F, G)>,
}

impl<A: Digital, F: Digital, B: Digital, G: Digital> Default for RCStreamZip<A, F, B, G> {
    fn default() -> Self {
        Self {
            a: RCStreamRelay::default(),
            b: RCStreamRelay::default(),
            out: RCStreamRelay::default(),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`RCStreamZip`].
pub struct In<A: Digital, F: Digital, B: Digital, G: Digital> {
    /// Data flowing in from the `a` source.
    pub a_data: Option<Item<A, F>>,
    /// Data flowing in from the `b` source.
    pub b_data: Option<Item<B, G>>,
    /// Ready flowing in from the downstream sink.
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [`RCStreamZip`].
pub struct Out<A: Digital, F: Digital, B: Digital, G: Digital> {
    /// Ready flowing out to the `a` source.
    pub a_ready: bool,
    /// Ready flowing out to the `b` source.
    pub b_ready: bool,
    /// The paired data flowing out to the downstream sink.
    pub data: Option<Item<(A, B), (F, G)>>,
}

impl<A: Digital, F: Digital, B: Digital, G: Digital> SynchronousIO for RCStreamZip<A, F, B, G> {
    type I = In<A, F, B, G>;
    type O = Out<A, F, B, G>;
    type Kernel = zip_kernel<A, F, B, G>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
// The `(O, D)` return tuple is the framework-mandated kernel
// signature (CLAUDE.md §2); with four generic parameters it trips
// `clippy::type_complexity`.  Factoring it behind an alias would
// obscure the convention every other widget follows.
#[allow(clippy::type_complexity)]
pub fn zip_kernel<A: Digital, F: Digital, B: Digital, G: Digital>(
    _cr: ClockReset,
    i: In<A, F, B, G>,
    q: Q<A, F, B, G>,
) -> (Out<A, F, B, G>, D<A, F, B, G>) {
    let mut d = D::<A, F, B, G>::dont_care();

    // Feed the input skid buffers.
    d.a.data = i.a_data;
    d.b.data = i.b_data;

    // Pair up only when both sides have an item AND the output buffer
    // can take it; then consume both in lockstep.
    let (have_a, item_a) = match q.a.data {
        Some(it) => (true, it),
        None => (false, Item::<A, F>::dont_care()),
    };
    let (have_b, item_b) = match q.b.data {
        Some(it) => (true, it),
        None => (false, Item::<B, G>::dont_care()),
    };
    let fire = have_a && have_b && q.out.ready;

    d.a.ready = fire;
    d.b.ready = fire;
    d.out.data = if fire {
        Some(Item::<(A, B), (F, G)> {
            data: (item_a.data, item_b.data),
            frame: (item_a.frame, item_b.frame),
        })
    } else {
        None
    };
    d.out.ready = i.ready;

    let o = Out::<A, F, B, G> {
        a_ready: q.a.ready,
        b_ready: q.b.ready,
        data: q.out.data,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcstream::bus::RCStream;
    use rhdl::core::sim::ResetOrData;

    type Zip = RCStreamZip<b8, bool, b4, ()>;

    fn a_item(v: u128, frame: bool) -> Item<b8, bool> {
        Item::<b8, bool> {
            data: bits::<8>(v),
            frame,
        }
    }
    fn b_item(v: u128) -> Item<b4, ()> {
        Item::<b4, ()> {
            data: bits::<4>(v),
            frame: (),
        }
    }

    fn q_with(
        a: Option<Item<b8, bool>>,
        b: Option<Item<b4, ()>>,
        out_ready: bool,
    ) -> Q<b8, bool, b4, ()> {
        Q::<b8, bool, b4, ()> {
            a: RCStream::<b8, bool> {
                data: a,
                ready: true,
            },
            b: RCStream::<b4, ()> {
                data: b,
                ready: true,
            },
            out: RCStream::<(b8, b4), (bool, ())> {
                data: None,
                ready: out_ready,
            },
        }
    }

    fn idle_in() -> In<b8, bool, b4, ()> {
        In::<b8, bool, b4, ()> {
            a_data: None,
            b_data: None,
            ready: true,
        }
    }

    /// Tier 1 — both sides present: a pair is formed carrying both
    /// payloads and both framing markers.
    #[test]
    fn pairs_when_both_sides_present() {
        let q = q_with(Some(a_item(0xAB, true)), Some(b_item(0x5)), true);
        let (_o, d) = zip_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
        match d.out.data {
            Some(it) => {
                assert_eq!(it.data.0.raw(), 0xAB);
                assert_eq!(it.data.1.raw(), 0x5);
                assert!(it.frame.0, "the a-side framing marker must be carried");
            }
            None => panic!("expected a pair"),
        }
        assert!(d.a.ready && d.b.ready, "both inputs consume in lockstep");
    }

    /// Tier 1 — one side missing: nothing fires, and neither input is
    /// consumed.  The side that does have data must NOT be dropped.
    #[test]
    fn does_not_fire_with_only_one_side() {
        let q = q_with(Some(a_item(0xAB, false)), None, true);
        let (_o, d) = zip_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
        assert!(d.out.data.is_none());
        assert!(
            !d.a.ready && !d.b.ready,
            "neither side may be consumed until both can pair"
        );
    }

    /// Tier 1 — output backpressure holds both inputs.  Without this the
    /// pair would be formed and dropped on the floor.
    #[test]
    fn output_backpressure_holds_both_inputs() {
        let q = q_with(Some(a_item(0xAB, false)), Some(b_item(0x5)), false);
        let (_o, d) = zip_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
        assert!(d.out.data.is_none(), "must not emit into a full buffer");
        assert!(
            !d.a.ready && !d.b.ready,
            "inputs must be held while the output cannot accept"
        );
    }

    /// Tier 1 — the ready signals for both sources come from their own
    /// skid buffers.
    #[test]
    fn per_source_ready_is_surfaced() {
        let mut q = q_with(None, None, true);
        q.a.ready = true;
        q.b.ready = false;
        let (o, _d) = zip_kernel::<b8, bool, b4, ()>(ClockReset::dont_care(), idle_in(), q);
        assert!(o.a_ready);
        assert!(!o.b_ready);
    }

    /// The LID requirement: no combinational path from any input to any
    /// output.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Zip::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed-loop end-to-end with the two sources running at
    /// *different* rates.  Pairs must still come out correctly aligned
    /// (a[k] with b[k]) and carry both framing markers: the faster
    /// source is simply backpressured until its partner catches up.
    #[test]
    fn pairs_align_despite_mismatched_source_rates() -> Result<(), RHDLError> {
        const COUNT: u128 = 24;
        let uut = Zip::default();
        let mut a_sent: u128 = 0;
        let mut b_sent: u128 = 0;
        let mut got: Vec<(u128, u128, bool)> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = !phase.is_multiple_of(4);
                if let Some(it) = output.data {
                    if sink_ready {
                        got.push((it.data.0.raw(), it.data.1.raw(), it.frame.0));
                    }
                }
                let mut input = In::<b8, bool, b4, ()> {
                    a_data: None,
                    b_data: None,
                    ready: sink_ready,
                };
                // `a` offers on every cycle; `b` only on every third —
                // a deliberate rate mismatch.
                if a_sent < COUNT && output.a_ready {
                    input.a_data = Some(a_item(a_sent, a_sent % 4 == 3));
                    a_sent += 1;
                }
                if b_sent < COUNT && output.b_ready && phase.is_multiple_of(3) {
                    input.b_data = Some(b_item(b_sent & 0xF));
                    b_sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 200_000)
        .for_each(drop);

        let expect: Vec<(u128, u128, bool)> =
            (0..COUNT).map(|k| (k, k & 0xF, k % 4 == 3)).collect();
        assert_eq!(
            got, expect,
            "pairs must stay index-aligned and carry both framing markers"
        );
        Ok(())
    }

    /// Build the open-loop stimulus for the codegen tiers.
    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, bool, b4, ()>)>> {
        (0..24u128)
            .map(|k| In::<b8, bool, b4, ()> {
                a_data: Some(a_item(k, k % 4 == 3)),
                b_data: Some(b_item(k & 0xF)),
                ready: k % 3 != 0,
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot of the widget's own module.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Zip::default();
        let desc = uut.descriptor("rcstream_zip".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_zip")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_zip(input wire [1:0] clock_reset, input wire [15:0] i, output wire [15:0] o);
               wire [47:0] od;
               wire [31:0] d;
               wire [31:0] q;
               assign o = od[15:0];
               rcstream_zip_a c0(.clock_reset(clock_reset), .i(d[10:0]), .o(q[10:0]));
               rcstream_zip_b c1(.clock_reset(clock_reset), .i(d[16:11]), .o(q[16:11]));
               rcstream_zip_out c2(.clock_reset(clock_reset), .i(d[31:17]), .o(q[31:17]));
               assign d = od[47:16];
               assign od = kernel_zip_kernel(clock_reset, i, q);
               function [47:0] kernel_zip_kernel(input reg [1:0] arg_0, input reg [15:0] arg_1, input reg [31:0] arg_2);
                     reg [9:0] r0;
                     reg [15:0] r1;
                     // d
                     reg [31:0] r2;
                     reg [4:0] r3;
                     // d
                     reg [31:0] r4;
                     reg [10:0] r5;
                     reg [31:0] r6;
                     reg [9:0] r7;
                     reg [0:0] r8;
                     reg [8:0] r9;
                     reg [9:0] r10;
                     reg [9:0] r11;
                     reg [0:0] r12;
                     reg [8:0] r13;
                     reg [5:0] r14;
                     reg [4:0] r15;
                     reg [0:0] r16;
                     reg [3:0] r17;
                     reg [4:0] r18;
                     reg [4:0] r19;
                     reg [0:0] r20;
                     reg [3:0] r21;
                     reg [0:0] r22;
                     reg [14:0] r23;
                     reg [0:0] r24;
                     reg [0:0] r25;
                     // d
                     reg [31:0] r26;
                     // d
                     reg [31:0] r27;
                     reg [7:0] r28;
                     reg [11:0] r29;
                     reg [0:0] r30;
                     reg [12:0] r31;
                     reg [12:0] r32;
                     reg [13:0] r33;
                     reg [12:0] r34;
                     reg [13:0] r35;
                     // d
                     reg [31:0] r36;
                     reg [0:0] r37;
                     // d
                     reg [31:0] r38;
                     reg [10:0] r39;
                     reg [0:0] r40;
                     reg [5:0] r41;
                     reg [0:0] r42;
                     reg [14:0] r43;
                     reg [13:0] r44;
                     reg [15:0] r45;
                     reg [15:0] r46;
                     reg [15:0] r47;
                     reg [47:0] r48;
                     reg [1:0] r49;
                     localparam l0 = 32'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 10'bXXXXXXXXX0;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 5'bXXXX0;
                     localparam l9 = 13'b0000000000000;
                     localparam l10 = 1'b1;
                     localparam l11 = 14'b00000000000000;
                     localparam l12 = 16'b0000000000000000;
                     begin
                        r49 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[9:0];
                        r2 = l0;
                        r2[9:0] = r0;
                        r3 = r1[14:10];
                        r4 = r2;
                        r4[15:11] = r3;
                        r5 = r6[10:0];
                        r7 = r5[9:0];
                        r8 = r7[9:9];
                        r9 = r7[8:0];
                        r10 = {r9, l1};
                        case (r8)
                           1'b1 : r11 = r10;
                           1'b0 : r11 = l4;
                        endcase
                        r12 = r11[0:0];
                        r13 = r11[9:1];
                        r14 = r6[16:11];
                        r15 = r14[4:0];
                        r16 = r15[4:4];
                        r17 = r15[3:0];
                        r18 = {r17, l5};
                        case (r16)
                           1'b1 : r19 = r18;
                           1'b0 : r19 = l8;
                        endcase
                        r20 = r19[0:0];
                        r21 = r19[4:1];
                        r22 = r12 & r20;
                        r23 = r6[31:17];
                        r24 = r23[14:14];
                        r25 = r22 & r24;
                        r26 = r4;
                        r26[10:10] = r25;
                        r27 = r26;
                        r27[16:16] = r25;
                        r28 = r13[7:0];
                        r29 = {r21, r28};
                        r30 = r13[8:8];
                        r31 = l9;
                        r31[11:0] = r29;
                        r32 = r31;
                        r32[12:12] = r30;
                        r34 = r32[12:0];
                        r33 = {l10, r34};
                        r35 = r25 ? r33 : l11;
                        r36 = r27;
                        r36[30:17] = r35;
                        r37 = r1[15:15];
                        r38 = r36;
                        r38[31:31] = r37;
                        r39 = r6[10:0];
                        r40 = r39[10:10];
                        r41 = r6[16:11];
                        r42 = r41[5:5];
                        r43 = r6[31:17];
                        r44 = r43[13:0];
                        r45 = l12;
                        r45[0:0] = r40;
                        r46 = r45;
                        r46[1:1] = r42;
                        r47 = r46;
                        r47[15:2] = r44;
                        r48 = {r38, r47};
                        kernel_zip_kernel = r48;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip on both RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = Zip::default();
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = Zip::default();
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_zip");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "c1481d5c6a629ebb89ae9dd4add31d1f79bdd84d7aeb3b5ee2c6c07965ad73aa"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_zip.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
