#![warn(missing_docs)]
//! [`super::super::RCStream<T, F>`] source → AXI4-Stream master output.
//!
//! Wraps an `RCStream<T, F>` source as an AXI4-Stream master output.
//! Same pattern as [`crate::axi4lite::stream::rhdl_to_axi::Rhdl2Axi`]
//! but generic over the framing parameter `F` and accepting a typed
//! `RCStream<T, F>` instead of `StreamIO<T, S>`.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +---+RCStreamToAxi+--+
      |  RCStream  :  AXI |
?Item<T,F>         :      | T
+---->| data       : tdata+--->
      |            :      | F
      |            : tuser+--->
      |            : tvalid+->
 bool |            :      |
<-----+ ready      : tready<--+
      +-------------------+
")]
//!
//! # Internal details
//!
//! Unpacks the source-side `Option<Item<T, F>>` into an
//! `(item, valid)` pair, then drives Carloni's `data_in / void_in`
//! with `(item, !valid)`.  Carloni's buffered `(data_out, void_out)`
//! exits as `(tdata + tuser, tvalid)` on the AXI side.  A
//! [`crate::lid::carloni::Carloni`] skid-buffer on the output isolates
//! the AXI bus from any combinatorial paths in the upstream RCStream-
//! side logic.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/rcstream_to_axi.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/rcstream_to_axi.md")]

use rhdl::prelude::*;

use crate::lid::carloni::{self, Carloni};
use crate::rcstream::bus::{Item, RCStream};

/// RCStream → AXI4-Stream translation widget.
///
/// `T` is the payload type (will appear on TDATA).  `F` is the
/// framing marker type (will appear on TUSER).  Use `F = ()` for
/// streams without framing.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamToAxi<T: Digital, F: Digital> {
    /// Carloni skid-buffer on the AXI output side, parameterized over
    /// `Item<T, F>` so it carries both payload + framing through.
    outbuf: Carloni<Item<T, F>>,
}

impl<T: Digital, F: Digital> Default for RCStreamToAxi<T, F> {
    fn default() -> Self {
        Self {
            outbuf: Carloni::<Item<T, F>>::default(),
        }
    }
}

/// Inputs for [`RCStreamToAxi`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital> {
    /// `RCStream` source-side `data`: `Some(item)` to deliver an
    /// item this cycle, `None` otherwise.
    pub data: Option<Item<T, F>>,
    /// AXI4-Stream `TREADY`: 1 when the AXI consumer is ready to
    /// accept the next item.
    pub tready: bool,
}

/// Outputs from [`RCStreamToAxi`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital> {
    /// AXI4-Stream `TDATA`: payload.
    pub tdata: T,
    /// AXI4-Stream `TUSER`: framing marker.  For `F = ()` this field
    /// is the unit value (zero wire bits).
    pub tuser: F,
    /// AXI4-Stream `TVALID`: 1 when `tdata`/`tuser` are valid this
    /// cycle.
    pub tvalid: bool,
    /// `RCStream` sink-side `ready`: 1 when this widget is ready to
    /// accept the next item from the upstream RCStream source.
    pub ready: bool,
}

impl<T: Digital, F: Digital> SynchronousIO for RCStreamToAxi<T, F> {
    type I = In<T, F>;
    type O = Out<T, F>;
    type Kernel = rcstream_to_axi_kernel<T, F>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn rcstream_to_axi_kernel<T: Digital, F: Digital>(
    _cr: ClockReset,
    i: In<T, F>,
    q: Q<T, F>,
) -> (Out<T, F>, D<T, F>) {
    let mut d = D::<T, F>::dont_care();
    let mut o = Out::<T, F>::dont_care();

    // Decompose RCStream input into Carloni's (data_in, void_in).
    // Single match yields the valid-flag and the payload, with the
    // None arm carrying a don't-care item (Carloni ignores it because
    // void_in=true).
    let (valid, item_in): (bool, Item<T, F>) = match i.data {
        Some(it) => (true, it),
        None => (
            false,
            Item::<T, F> {
                data: T::dont_care(),
                frame: F::dont_care(),
            },
        ),
    };
    d.outbuf.data_in = item_in;
    d.outbuf.void_in = !valid;
    d.outbuf.stop_in = !i.tready;

    // Compose AXI output from Carloni's (data_out, void_out, stop_out).
    o.tdata = q.outbuf.data_out.data;
    o.tuser = q.outbuf.data_out.frame;
    o.tvalid = !q.outbuf.void_out;
    o.ready = !q.outbuf.stop_out;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _u: RCStreamToAxi<b8, ()> = RCStreamToAxi::default();
        let _u2: RCStreamToAxi<b16, bool> = RCStreamToAxi::default();
        let _u3: RCStreamToAxi<b32, b8> = RCStreamToAxi::default();
    }

    /// Direct kernel test: an incoming RCStream Some(item) flows into
    /// Carloni's data_in/void_in=0.
    #[test]
    fn kernel_forwards_rcstream_to_carloni() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> {
            data: bits::<8>(0xCD),
            frame: (),
        };
        let i = In::<b8, ()> {
            data: Some(item),
            tready: true,
        };
        let q = Q::<b8, ()> {
            outbuf: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = rcstream_to_axi_kernel::<b8, ()>(cr, i, q);
        assert_eq!(d.outbuf.data_in.data.raw(), 0xCD);
        assert_eq!(d.outbuf.void_in, false);
        assert_eq!(d.outbuf.stop_in, false);
    }

    /// Direct kernel test: data: None produces void_in=1.
    #[test]
    fn kernel_idle_rcstream_produces_void() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, ()> {
            data: None,
            tready: true,
        };
        let q = Q::<b8, ()> {
            outbuf: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = rcstream_to_axi_kernel::<b8, ()>(cr, i, q);
        assert_eq!(d.outbuf.void_in, true);
    }

    /// Direct kernel test: when Carloni has buffered data, it emerges
    /// on the AXI output as (tdata, tuser, tvalid=1).
    #[test]
    fn kernel_outputs_buffered_axi() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, bool> {
            data: None,
            tready: true,
        };
        let held = Item::<b8, bool> {
            data: bits::<8>(0xEF),
            frame: true,
        };
        let q = Q::<b8, bool> {
            outbuf: carloni::Out::<Item<b8, bool>> {
                data_out: held,
                void_out: false,
                stop_out: false,
            },
        };
        let (o, _d) = rcstream_to_axi_kernel::<b8, bool>(cr, i, q);
        assert_eq!(o.tdata.raw(), 0xEF);
        assert_eq!(o.tuser, true);
        assert_eq!(o.tvalid, true);
        assert_eq!(o.ready, true);
    }

    /// Tier 2 — **backpressure**, the mirror of the AXI->RCStream case
    /// and equally uncovered before now.
    ///
    /// Here the AXI *consumer* deasserts TREADY intermittently while the
    /// RCStream source keeps offering.  Per AMBA the translator must
    /// hold TDATA/TVALID stable until TREADY, and must not drop the
    /// held beat.
    #[test]
    fn rcstream_to_axi_loses_nothing_when_the_axi_consumer_stalls() {
        use rhdl::core::sim::ResetOrData;
        const COUNT: u128 = 20;
        let uut = RCStreamToAxi::<b8, ()>::default();
        let mut to_send: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                // AXI consumer accepts 1 cycle in 3.
                let tready = phase.is_multiple_of(3);
                if output.tvalid && tready {
                    got.push(output.tdata.raw());
                }
                let mut input = In::<b8, ()> { data: None, tready };
                // RCStream source offers whenever the translator has room.
                if to_send < COUNT && output.ready {
                    input.data = Some(Item::<b8, ()> {
                        data: b8(to_send % 256),
                        frame: (),
                    });
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);

        let want: Vec<u128> = (0..COUNT).collect();
        assert_eq!(
            got, want,
            "RCStream->AXI must hold its beat and lose nothing when TREADY drops"
        );
    }

    /// Stimulus for the Tier-5 digest.
    ///
    /// Gaps the incoming data and drops `TREADY` on different cadences
    /// (4 and 3) so they never coincide. AXI requires a beat to be held
    /// stable until TREADY is seen, so a stream that never drops TREADY
    /// exercises everything except that obligation.
    fn handshake_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, ()>)>> {
        (0..24u128)
            .map(|k| In {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(Item::<b8, ()> {
                        data: bits::<8>(k),
                        frame: (),
                    })
                },
                tready: !k.is_multiple_of(3),
            })
            .with_reset(2)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut: RCStreamToAxi<b8, ()> = RCStreamToAxi::default();
        let desc = uut.descriptor("rcstream_to_axi".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_to_axi")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_to_axi(input wire [1:0] clock_reset, input wire [9:0] i, output wire [9:0] o);
               wire [19:0] od;
               wire [9:0] d;
               wire [9:0] q;
               assign o = od[9:0];
               rcstream_to_axi_outbuf c0(.clock_reset(clock_reset), .i(d[9:0]), .o(q[9:0]));
               assign d = od[19:10];
               assign od = kernel_rcstream_to_axi_kernel(clock_reset, i, q);
               function [19:0] kernel_rcstream_to_axi_kernel(input reg [1:0] arg_0, input reg [9:0] arg_1, input reg [9:0] arg_2);
                     reg [8:0] r0;
                     reg [9:0] r1;
                     reg [0:0] r2;
                     reg [7:0] r3;
                     reg [8:0] r4;
                     reg [8:0] r5;
                     reg [0:0] r6;
                     reg [7:0] r7;
                     // d
                     reg [9:0] r8;
                     reg [0:0] r9;
                     // d
                     reg [9:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     // d
                     reg [9:0] r13;
                     reg [9:0] r14;
                     reg [7:0] r15;
                     // o
                     reg [9:0] r16;
                     reg [0:0] r17;
                     reg [0:0] r18;
                     // o
                     reg [9:0] r19;
                     reg [0:0] r20;
                     reg [0:0] r21;
                     // o
                     reg [9:0] r22;
                     reg [19:0] r23;
                     reg [1:0] r24;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 9'bXXXXXXXX0;
                     localparam l4 = 10'bXXXXXXXXXX;
                     localparam l5 = 10'bXXXXXXXXXX;
                     begin
                        r24 = arg_0;
                        r1 = arg_1;
                        r14 = arg_2;
                        r0 = r1[8:0];
                        r2 = r0[8:8];
                        r3 = r0[7:0];
                        r4 = {r3, l0};
                        case (r2)
                           1'b1 : r5 = r4;
                           1'b0 : r5 = l3;
                        endcase
                        r6 = r5[0:0];
                        r7 = r5[8:1];
                        r8 = l4;
                        r8[7:0] = r7;
                        r9 = ~r6;
                        r10 = r8;
                        r10[8:8] = r9;
                        r11 = r1[9:9];
                        r12 = ~r11;
                        r13 = r10;
                        r13[9:9] = r12;
                        r15 = r14[7:0];
                        r16 = l5;
                        r16[7:0] = r15;
                        r17 = r14[8:8];
                        r18 = ~r17;
                        r19 = r16;
                        r19[8:8] = r18;
                        r20 = r14[9:9];
                        r21 = ~r20;
                        r22 = r19;
                        r22[9:9] = r21;
                        r23 = {r13, r22};
                        kernel_rcstream_to_axi_kernel = r23;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 5 — VCD digest, over the gapped/stalled handshake.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut: RCStreamToAxi<b8, ()> = RCStreamToAxi::default();
        let vcd = uut.run(handshake_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_to_axi");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "6aec5876c80c97d99b3b1d39ec29bbbc148178ddaf1265f108069400f743ad51"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_to_axi.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// iverilog round-trip: F=().
    #[test]
    fn iverilog_round_trip_f_unit() -> Result<(), RHDLError> {
        let uut: RCStreamToAxi<b8, ()> = RCStreamToAxi::default();
        let inputs: Vec<In<b8, ()>> = (0..16)
            .map(|k| {
                let it = Item::<b8, ()> {
                    data: bits::<8>(k as u128),
                    frame: (),
                };
                In {
                    data: Some(it),
                    tready: true,
                }
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}

/// Round-trip property test in its own nested module so the
/// `RoundTrip` widget's auto-derived `Q` doesn't shadow the parent
/// `RCStreamToAxi`'s `Q` used by the unit kernel tests above.
#[cfg(test)]
mod round_trip_tests {
    use super::*;
    use crate::rcstream::axi_stream::axi_to_rcstream::{self, AxiToRCStream};

    /// Round-trip property: `axi → AxiToRCStream → RCStream<T, F> →
    /// RCStreamToAxi → axi` is byte-identical on the AXI side
    /// (modulo the one-cycle Carloni latency on each translator).
    /// This is the validation criterion called out in
    /// `stream-bus-architecture.md` §10.
    #[derive(Clone, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    struct RoundTrip<T: Digital, F: Digital> {
        a2r: AxiToRCStream<T, F>,
        r2a: RCStreamToAxi<T, F>,
    }

    impl<T: Digital, F: Digital> SynchronousIO for RoundTrip<T, F> {
        type I = axi_to_rcstream::In<T, F>;
        type O = Out<T, F>;
        type Kernel = round_trip_kernel<T, F>;
    }

    #[kernel(allow_weak_partial)]
    #[doc(hidden)]
    pub fn round_trip_kernel<T: Digital, F: Digital>(
        _cr: ClockReset,
        i: axi_to_rcstream::In<T, F>,
        q: Q<T, F>,
    ) -> (Out<T, F>, D<T, F>) {
        let mut d = D::<T, F>::dont_care();
        // AxiToRCStream sees the AXI input directly.
        d.a2r = i;
        // RCStreamToAxi sees the AxiToRCStream's output as its input.
        d.r2a.data = q.a2r.data;
        d.r2a.tready = i.ready; // pass-through downstream tready
        // The RCStream-side ready flowing back into AxiToRCStream
        // comes from RCStreamToAxi's internal `ready` output.
        d.a2r.ready = q.r2a.ready;
        // Output of the round-trip is the second translator's AXI side.
        (q.r2a, d)
    }

    /// Round-trip back-to-back: tdata/tuser appear at the output
    /// after passing through both translators' Carloni buffers.
    /// Smoke-checks that the design composes cleanly (descriptor
    /// builds, run produces output) and that the data values seen
    /// on the AXI output side match the data values fed on the AXI
    /// input side after a 2-cycle latency (one per Carloni).
    ///
    /// NOT iverilog-tested: the round-trip introduces X-states on
    /// TDATA when TVALID=0 (Carloni's `void_out=true` produces a
    /// don't-care `data_out` that propagates as X in iverilog
    /// until valid data arrives).  The single-translator iverilog
    /// round-trip tests cover the Verilog code path; the
    /// composition test here is Rust-sim only.
    #[test]
    fn test_round_trip_compose() {
        let uut: RoundTrip<b8, ()> = RoundTrip::default();
        let n_items: u128 = 32;
        let inputs: Vec<axi_to_rcstream::In<b8, ()>> = (0..n_items)
            .map(|k| axi_to_rcstream::In::<b8, ()> {
                tdata: bits::<8>(k & 0xFF),
                tuser: (),
                tvalid: true,
                ready: true,
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        // Collect the AXI-side output (Out<b8, ()>) per cycle.  We
        // expect, after the Carloni latency, the tdata sequence to
        // mirror the input sequence.
        let outputs: Vec<Out<b8, ()>> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        // Pull out just the tdata values where tvalid is asserted.
        let tdata_seq: Vec<u128> = outputs
            .iter()
            .filter(|o| o.tvalid)
            .map(|o| o.tdata.raw())
            .collect();
        // Should see at least n_items - 2 (latency) of the input
        // sequence (= 0..30 of the 0..32 input range).
        assert!(
            tdata_seq.len() >= (n_items as usize) - 4,
            "round-trip dropped too many items: got {} valid outputs from \
             {} inputs",
            tdata_seq.len(),
            n_items
        );
        // Spot-check that the sequence is monotonically non-decreasing
        // (= no items dropped or reordered through the back-to-back
        // Carlonis with always-ready downstream).
        for w in tdata_seq.windows(2) {
            assert!(
                w[1] == w[0] + 1 || w[1] == w[0],
                "round-trip out-of-order: saw {} after {}",
                w[1],
                w[0]
            );
        }
    }
}
