#![warn(missing_docs)]
//! `RCStreamRelay<T, F>` — a Carloni relay station with the typed
//! [`RCStream`] interface.
//!
//! Wraps the LID-paper-faithful [`crate::lid::carloni::Carloni`]
//! widget — same skid-buffer FSM, same throughput, same one-cycle
//! latency — but presents the typed `RCStream<T, F>` I/O instead of
//! the 3-signal `data/void/stop` Carloni interface.  This is the
//! canonical pipeline-stage primitive for `RCStream` connections.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!("
     +-+RCStreamRelay+-----+
?T,F |                     | ?T,F
+--->+ data           data +--->
     |                     |
 <---+ ready         ready |<---+
     +---------------------+
")]
//!
//! # Design property
//!
//! Per Carloni's LID theorem (DAC 1999, *Proceedings of the IEEE*
//! 2015 retrospective), inserting a relay station anywhere on a
//! latency-insensitive connection adds one cycle of latency without
//! changing throughput or functional behavior.  This is the formal
//! basis for sound auto-pipelining at inter-kernel boundaries: the
//! auto-pipeliner can place an `RCStreamRelay` on any `RCStream`
//! connection with no hazard analysis required.
//!
//! # Internals
//!
//! Translates between `RCStream<T, F>` and the Carloni `data/void/stop`
//! 3-signal interface:
//!
//! ```text
//!   RCStream            Carloni
//!   data: Option<Item>  ←→  (data: Item, void: bool)   void = data.is_none()
//!   ready: bool         ←→  stop: bool                  stop = !ready
//! ```
//!
//! See [`crate::lid::carloni`] for the underlying FSM diagram and
//! the original-paper reference.
//!
//! # When to use
//!
//! - Any time an `RCStream` connection's TVALID/TREADY combinational
//!   path is a timing-closure concern.  Insert the relay; the LID
//!   theorem says throughput is unchanged.
//! - At inter-kernel boundaries where the auto-pipeliner needs a
//!   sound cut point.  Relay insertion at `RCStream` boundaries is
//!   guaranteed-correct without hazard analysis (per the design
//!   plan, this is the auto-pipeliner's preferred cut point).
//! - Anywhere a vendor's IP block expects a registered Ready/Valid
//!   handshake (to avoid combinational paths through the IP boundary).
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_relay.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_relay.md")]

use rhdl::prelude::*;

use crate::lid::carloni::Carloni;
use crate::rcstream::bus::{Item, RCStream};

/// A Carloni relay station with the typed [`RCStream`] interface.
///
/// One cycle of latency, same throughput.  Pure thin wrapper around
/// [`Carloni<Item<T, F>>`] — see module docs for the encoding bridge.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamRelay<T: Digital, F: Digital> {
    /// The underlying Carloni skid-buffer, parameterized over
    /// `Item<T, F>` (the bus's payload type).
    inner: Carloni<Item<T, F>>,
}

impl<T: Digital, F: Digital> Default for RCStreamRelay<T, F> {
    fn default() -> Self {
        Self {
            inner: Carloni::<Item<T, F>>::default(),
        }
    }
}

impl<T: Digital, F: Digital> SynchronousIO for RCStreamRelay<T, F> {
    type I = RCStream<T, F>;
    type O = RCStream<T, F>;
    type Kernel = relay_kernel<T, F>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn relay_kernel<T: Digital, F: Digital>(
    _cr: ClockReset,
    i: RCStream<T, F>,
    q: Q<T, F>,
) -> (RCStream<T, F>, D<T, F>) {
    let mut d = D::<T, F>::dont_care();
    let mut o = RCStream::<T, F>::dont_care();

    // Decompose RCStream `i` (incoming side) into Carloni's
    // (data_in, void_in, stop_in).  Single match yields the
    // valid-flag and the payload, with the None arm carrying a
    // don't-care payload (Carloni ignores it because void_in=true).
    // Mirrors the existing `stream_buffer::option_carloni_kernel`
    // pattern.  Requires `#[kernel(allow_weak_partial)]` so RHDL's
    // kernel-coverage tracker accepts the don't-care leaves of
    // `Item<T, F>` in the None arm.
    let (data_valid, item_in): (bool, Item<T, F>) = match i.data {
        Some(it) => (true, it),
        None => (
            false,
            Item::<T, F> {
                data: T::dont_care(),
                frame: F::dont_care(),
            },
        ),
    };
    d.inner.data_in = item_in;
    d.inner.void_in = !data_valid;
    d.inner.stop_in = !i.ready;

    // Compose RCStream `o` (outgoing side) from Carloni's
    // (data_out, void_out, stop_out).
    o.data = if q.inner.void_out {
        None
    } else {
        Some(q.inner.data_out)
    };
    o.ready = !q.inner.stop_out;

    (o, d)
}

#[cfg(test)]
mod tests {
    // `carloni` is referenced only from these tests, so the
    // import is scoped to them -- at file level it reads as
    // unused when the crate is checked without test targets.
    use super::*;
    use crate::lid::carloni;

    /// Stimulus for the Tier-5 digest.
    ///
    /// Deliberately **stalls**: `ready` is withheld on one cycle in
    /// three. A skid buffer exists to absorb stalls, so a digest taken
    /// over an always-ready stream would anchor the one trajectory that
    /// never uses the buffer, and would stay green through a regression
    /// in exactly the logic this widget is for.
    fn digest_stream() -> impl Iterator<Item = TimedSample<(ClockReset, RCStream<b8, ()>)>> {
        (0..24u128)
            .map(|k| RCStream::<b8, ()> {
                data: Some(Item::<b8, ()> {
                    data: bits::<8>(k),
                    frame: (),
                }),
                ready: !k.is_multiple_of(3),
            })
            .with_reset(2)
            .clock_pos_edge(100)
    }

    /// A relay with no items in flight should idle: data out = None,
    /// ready out = false (Carloni starts in Run with stop_out=true to
    /// signal "I'm not ready yet, don't send").
    ///
    /// More importantly: this confirms the type infrastructure
    /// composes — `RCStreamRelay<T, F>` builds a valid `Synchronous`
    /// widget that the framework accepts.
    #[test]
    fn relay_default_construction() {
        let _r: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let _r2: RCStreamRelay<b32, bool> = RCStreamRelay::default();
        let _r3: RCStreamRelay<b16, b8> = RCStreamRelay::default();
    }

    /// Direct kernel test: idle in → idle out (after 1-cycle latency
    /// the relay still has no data to deliver).
    #[test]
    fn relay_kernel_idle() {
        let cr = ClockReset::dont_care();
        let i = RCStream::<b8, ()> {
            data: None,
            ready: true,
        };
        let q = Q::<b8, ()> {
            inner: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true, // no data in main_ff
                stop_out: false,
            },
        };
        let (o, _d) = relay_kernel::<b8, ()>(cr, i, q);
        assert!(o.data.is_none());
        assert_eq!(o.ready, true); // !stop_out = !false = true
    }

    /// Direct kernel test: when Carloni has buffered data
    /// (q.inner.void_out = false), the relay output's data is
    /// Some(item).
    #[test]
    fn relay_kernel_data_held() {
        let cr = ClockReset::dont_care();
        let i = RCStream::<b8, ()> {
            data: None,
            ready: true,
        };
        let held = Item::<b8, ()> {
            data: bits::<8>(0xAB),
            frame: (),
        };
        let q = Q::<b8, ()> {
            inner: carloni::Out::<Item<b8, ()>> {
                data_out: held,
                void_out: false, // data valid
                stop_out: false,
            },
        };
        let (o, _d) = relay_kernel::<b8, ()>(cr, i, q);
        match o.data {
            Some(it) => assert_eq!(it.data.raw(), 0xAB),
            None => panic!("expected Some(item) when void_out=false"),
        }
        assert_eq!(o.ready, true);
    }

    /// Direct kernel test: an incoming item is forwarded into
    /// Carloni's `data_in`/`void_in`.
    #[test]
    fn relay_kernel_forwards_item_to_carloni() {
        let cr = ClockReset::dont_care();
        let it = Item::<b8, ()> {
            data: bits::<8>(0x55),
            frame: (),
        };
        let i = RCStream::<b8, ()> {
            data: Some(it),
            ready: false, // downstream not ready → stop_in=true
        };
        let q = Q::<b8, ()> {
            inner: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = relay_kernel::<b8, ()>(cr, i, q);
        assert_eq!(d.inner.data_in.data.raw(), 0x55);
        assert_eq!(d.inner.void_in, false); // is_none() = false → void = false
        assert_eq!(d.inner.stop_in, true); // !ready = true
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Snapshots the top module only: the sub-modules are the Carloni
    /// primitive's own emission, covered by its snapshot, and including
    /// them here would make this test fail for changes that have nothing
    /// to do with the relay.
    ///
    /// This replaces a `descriptor()`-only smoke test. That test proved
    /// the `Synchronous` derive composed with the wrapped Carloni, which
    /// is worth knowing but is not Tier 3 — it could not detect a change
    /// in what the relay actually emits.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let desc = uut.descriptor("rcstream_relay".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_relay")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_relay(input wire [1:0] clock_reset, input wire [9:0] i, output wire [9:0] o);
               wire [19:0] od;
               wire [9:0] d;
               wire [9:0] q;
               assign o = od[9:0];
               rcstream_relay_inner c0(.clock_reset(clock_reset), .i(d[9:0]), .o(q[9:0]));
               assign d = od[19:10];
               assign od = kernel_relay_kernel(clock_reset, i, q);
               function [19:0] kernel_relay_kernel(input reg [1:0] arg_0, input reg [9:0] arg_1, input reg [9:0] arg_2);
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
                     reg [0:0] r15;
                     reg [7:0] r16;
                     reg [8:0] r17;
                     reg [7:0] r18;
                     reg [8:0] r19;
                     // o
                     reg [9:0] r20;
                     reg [0:0] r21;
                     reg [0:0] r22;
                     // o
                     reg [9:0] r23;
                     reg [19:0] r24;
                     reg [1:0] r25;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 9'bXXXXXXXX0;
                     localparam l4 = 10'bXXXXXXXXXX;
                     localparam l5 = 1'b1;
                     localparam l6 = 9'b000000000;
                     localparam l7 = 10'bXXXXXXXXXX;
                     begin
                        r25 = arg_0;
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
                        r15 = r14[8:8];
                        r16 = r14[7:0];
                        r18 = r16[7:0];
                        r17 = {l5, r18};
                        r19 = r15 ? l6 : r17;
                        r20 = l7;
                        r20[8:0] = r19;
                        r21 = r14[9:9];
                        r22 = ~r21;
                        r23 = r20;
                        r23[9:9] = r22;
                        r24 = {r13, r23};
                        kernel_relay_kernel = r24;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let vcd = uut.run(digest_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_relay");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "604f9a06e19cc971b3318a09cdd90e5fa6b0c837e4ae7da769346a722c90d095"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_relay.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// Tier 2 — the relay under sustained backpressure.
    ///
    /// The relay's insertion-invariance is covered at composition level
    /// in `tests/rcstream_relay_insertion.rs`, but the widget's own
    /// suite only ever drove `ready: true`.  A skid buffer exists to
    /// absorb stalls, so a test that never stalls exercises everything
    /// except its reason for existing.
    /// Driven through [`crate::rcstream::testing`] rather than a
    /// hand-rolled `run_fn`. Same claim, same 1-in-3 sink cadence; the
    /// fixture owns the reset/collect/terminate bookkeeping and
    /// distinguishes "stalled" from "delivered the wrong thing", which
    /// the hand-rolled version could not.
    #[test]
    fn relay_loses_nothing_under_backpressure() {
        use crate::rcstream::testing::{Cadence, drive};
        const COUNT: u128 = 24;
        let uut = RCStreamRelay::<b8, bool>::default();
        let want: Vec<Item<b8, bool>> = (0..COUNT)
            .map(|k| Item::<b8, bool> {
                data: bits::<8>(k),
                frame: k % 8 == 7,
            })
            .collect();
        let out = drive::<_, b8, bool, b8>(&uut, &want, Cadence::Periodic(3), 20_000);
        out.assert_exactly(&want);
    }

    /// The same relay against a **data-gated** sink — one that withholds
    /// `ready` whenever it sees nothing on the wire.
    ///
    /// A skid buffer presents `None` while empty, so this is the shape
    /// that deadlocks a widget which waits for a downstream it never
    /// showed anything to. One line, because the fixture exists.
    #[test]
    fn relay_survives_a_data_gated_sink() {
        use crate::rcstream::testing::assert_lossless;
        let uut = RCStreamRelay::<b8, ()>::default();
        let want: Vec<Item<b8, ()>> = (0..24u128)
            .map(|k| Item::<b8, ()> {
                data: bits::<8>(k),
                frame: (),
            })
            .collect();
        assert_lossless(&uut, &want);
    }

    /// iverilog round-trip: the relay's emitted Verilog matches the
    /// Rust simulation exactly.
    #[test]
    fn relay_iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        let inputs: Vec<RCStream<b8, ()>> = (0..16)
            .map(|k| {
                let it = Item::<b8, ()> {
                    data: bits::<8>(k as u128),
                    frame: (),
                };
                RCStream::<b8, ()> {
                    data: Some(it),
                    ready: true,
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

    /// Round-trip with framing parameter `F = bool` (TLAST-equivalent).
    /// Verifies the relay's typed-framing flow-through works.
    #[test]
    fn relay_with_framing_round_trip() -> Result<(), RHDLError> {
        let uut: RCStreamRelay<b8, bool> = RCStreamRelay::default();
        let inputs: Vec<RCStream<b8, bool>> = (0..16)
            .map(|k| {
                let it = Item::<b8, bool> {
                    data: bits::<8>(k as u128),
                    frame: k == 15, // last item carries TLAST = true
                };
                RCStream::<b8, bool> {
                    data: Some(it),
                    ready: true,
                }
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        // NTL as well as RTL: the RTL form skips the Stage-3 NTL passes,
        // so an RTL-only round-trip cannot detect a bug in those passes.
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}
