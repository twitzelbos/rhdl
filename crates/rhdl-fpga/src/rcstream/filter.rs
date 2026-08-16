#![warn(missing_docs)]
//! Drop items from an [`RCStream`] that fail a combinational predicate.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+RCStreamFilter+-+
 ?T   |                  |  ?T
+---->| data        data +---->
      |                  |
<-----+ ready      ready |<----+
      |                  |
      +------------------+
")]
//!
//!# Framing: why the predicate sees the whole `Item`
//!
//! The predicate is `fn(ClockReset, Item<T, F>) -> bool`, not
//! `fn(ClockReset, T) -> bool`.  This is deliberate.
//!
//! Filtering **destroys framing information**.  If `F` is a
//! TLAST-equivalent end-of-frame marker and the predicate drops the
//! item carrying `frame = true`, the frame simply never ends, and every
//! downstream consumer that counts frames is now permanently wrong.  No
//! type check catches that — it is a data-dependent, run-time property.
//!
//! Handing the predicate the whole [`Item`] makes the hazard *visible
//! at the call site*: the author of the predicate can see the framing
//! marker and must decide what to do with it (typically
//! `it.frame || keep(it.data)`, which preserves frame boundaries by
//! never dropping a marker-carrying item).  This is exactly the kind of
//! out-of-band convention the typed bus exists to eliminate.
//!
//! [`super::map`] takes the payload alone, because a `map` cannot drop
//! anything and therefore cannot destroy framing.
//!
//!# Dropped items advance the buffer
//!
//! When the predicate rejects an item, this widget asserts `ready` to
//! its internal skid buffer **regardless of the downstream `ready`**:
//!
//! ```text
//! d.input.ready = i.ready || dropping
//! ```
//!
//! This matters.  The bus contract in [`super::bus`] permits a sink's
//! `ready` to depend combinationally on `data.is_some()` — a sink may
//! legitimately only assert `ready` when it can see an item.  A
//! rejected item produces `data = None` downstream, so such a sink
//! never asserts `ready`, and if the widget waited for it the dropped
//! item would sit in the buffer forever and the stream would deadlock.
//! Consuming rejected items ourselves is what makes the widget correct
//! against every conforming sink rather than only against
//! unconditionally-ready ones.
//!
//!# Internals
//!
//! An [`RCStreamRelay`] (Carloni skid buffer) feeding a combinational
//! [`Func`] predicate.  No combinational path from any input to any
//! output — verified by a [`drc::no_combinatorial_paths`] test.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_filter.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_filter.md")]

use rhdl::{
    core::{ClockReset, DigitalFn, DigitalFn2, RHDLError},
    prelude::*,
};

use super::bus::{Item, RCStream};
use super::relay::RCStreamRelay;

/// Drop items from an [`RCStream`] that fail a combinational predicate.
///
/// `T` is the payload type and `F` the framing-marker type.  Items that
/// pass the predicate are forwarded unchanged, framing included.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamFilter<T: Digital, F: Digital> {
    /// Carloni skid buffer isolating the upstream handshake.
    input: RCStreamRelay<T, F>,
    /// The combinational predicate, over the whole item.
    pred: Func<Item<T, F>, bool>,
}

impl<T: Digital, F: Digital> RCStreamFilter<T, F> {
    /// Construct an [`RCStreamFilter`] from a synthesizable predicate.
    ///
    /// `K` must be a `#[kernel]` function with the signature
    /// `fn(ClockReset, Item<T, F>) -> bool`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = Item<T, F>, O = bool>,
    {
        Ok(Self {
            input: RCStreamRelay::default(),
            pred: Func::try_new::<K>()?,
        })
    }
}

impl<T: Digital, F: Digital> SynchronousIO for RCStreamFilter<T, F> {
    type I = RCStream<T, F>;
    type O = RCStream<T, F>;
    type Kernel = filter_kernel<T, F>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn filter_kernel<T: Digital, F: Digital>(
    _cr: ClockReset,
    i: RCStream<T, F>,
    q: Q<T, F>,
) -> (RCStream<T, F>, D<T, F>) {
    let mut d = D::<T, F>::dont_care();
    d.input.data = i.data;

    // Present the buffered item (if any) to the predicate.
    let (have, item) = match q.input.data {
        Some(it) => (true, it),
        None => (false, Item::<T, F>::dont_care()),
    };
    d.pred = item;

    let keep = have && q.pred;
    let dropping = have && !q.pred;

    // Consume rejected items ourselves — see the module docs.  Waiting
    // for `i.ready` here would deadlock against any sink that gates its
    // ready on seeing data.
    d.input.ready = i.ready || dropping;

    let o = RCStream::<T, F> {
        data: if keep { Some(item) } else { None },
        ready: q.input.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::ResetOrData;

    /// Keep even payloads, and never drop an end-of-frame marker — the
    /// idiom the module docs recommend for framing-safe filtering.
    #[kernel]
    fn keep_even(_cr: ClockReset, it: Item<b8, bool>) -> bool {
        it.frame || ((it.data & bits::<8>(1)) == bits::<8>(0))
    }

    fn item(v: u128, frame: bool) -> Item<b8, bool> {
        Item::<b8, bool> {
            data: bits::<8>(v),
            frame,
        }
    }

    fn q_with(data: Option<Item<b8, bool>>, pred: bool, ready: bool) -> Q<b8, bool> {
        Q::<b8, bool> {
            input: RCStream::<b8, bool> { data, ready },
            pred,
        }
    }

    /// Tier 1 — a passing item is forwarded unchanged, framing included.
    #[test]
    fn passing_item_is_forwarded_intact() {
        let q = q_with(Some(item(0x20, true)), true, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, d) = filter_kernel::<b8, bool>(ClockReset::dont_care(), i, q);
        match o.data {
            Some(it) => {
                assert_eq!(it.data.raw(), 0x20);
                assert!(it.frame, "framing must survive the filter");
            }
            None => panic!("expected the item to pass"),
        }
        assert_eq!(d.pred.data.raw(), 0x20, "predicate sees the whole item");
    }

    /// Tier 1 — a rejected item produces no output.
    #[test]
    fn rejected_item_produces_no_output() {
        let q = q_with(Some(item(0x21, false)), false, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, _d) = filter_kernel::<b8, bool>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
    }

    /// Tier 1 — **the deadlock guard**.  A rejected item must be
    /// consumed from the buffer even when the downstream sink is not
    /// ready, because a sink is allowed to withhold `ready` until it
    /// sees data — and a rejected item shows it none.
    #[test]
    fn rejected_item_advances_buffer_without_downstream_ready() {
        let q = q_with(Some(item(0x21, false)), false, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false, // downstream is NOT ready
        };
        let (_o, d) = filter_kernel::<b8, bool>(ClockReset::dont_care(), i, q);
        assert!(
            d.input.ready,
            "a dropped item must be consumed regardless of downstream ready, \
             or the stream deadlocks against a sink that gates ready on data"
        );
    }

    /// Tier 1 — the complement: a *passing* item must NOT be consumed
    /// while downstream is stalled, or it would be lost.
    #[test]
    fn passing_item_is_held_while_downstream_stalls() {
        let q = q_with(Some(item(0x20, false)), true, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (o, d) = filter_kernel::<b8, bool>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_some(), "the item must still be presented");
        assert!(
            !d.input.ready,
            "a passing item must be held until downstream takes it"
        );
    }

    /// Tier 1 — an empty buffer drops nothing and forwards nothing.
    #[test]
    fn empty_buffer_is_inert() {
        let q = q_with(None, false, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (o, d) = filter_kernel::<b8, bool>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
        assert!(
            !d.input.ready,
            "an empty buffer must not be told to advance"
        );
    }

    /// The LID requirement: no combinational path from any input to any
    /// output.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = RCStreamFilter::<b8, bool>::try_new::<keep_even>()?;
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed-loop end-to-end against a **data-gated sink**:
    /// one whose `ready` is only asserted when it can see an item.  The
    /// bus contract permits exactly that, and it is the sink that would
    /// deadlock a filter which waited for downstream before discarding
    /// a rejected item.  Every accepted item must emerge, in order,
    /// and every rejected one must be gone.
    #[test]
    fn stream_filters_against_a_data_gated_sink() -> Result<(), RHDLError> {
        const COUNT: u128 = 40;
        let uut = RCStreamFilter::<b8, bool>::try_new::<keep_even>()?;
        let mut to_send: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                // The sink only asserts ready when it actually sees data.
                let sink_ready = output.data.is_some();
                if let Some(it) = output.data {
                    got.push(it.data.raw());
                }
                let mut input = RCStream::<b8, bool> {
                    data: None,
                    ready: sink_ready,
                };
                if to_send < COUNT && output.ready {
                    input.data = Some(item(to_send, to_send % 8 == 7));
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 100_000)
        .for_each(drop);

        // The predicate keeps evens, and never drops a frame marker.
        let expect: Vec<u128> = (0..COUNT).filter(|k| k % 8 == 7 || k % 2 == 0).collect();
        assert_eq!(
            got, expect,
            "filtered stream must match the predicate exactly, with no deadlock"
        );
        Ok(())
    }

    /// Build the open-loop stimulus for the codegen tiers.
    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, RCStream<b8, bool>)>> {
        (0..24u128)
            .map(|k| RCStream::<b8, bool> {
                data: Some(item(k, k % 8 == 7)),
                ready: k % 3 != 0,
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot of the widget's own module.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = RCStreamFilter::<b8, bool>::try_new::<keep_even>()?;
        let desc = uut.descriptor("rcstream_filter".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_filter")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_filter(input wire [1:0] clock_reset, input wire [10:0] i, output wire [10:0] o);
               wire [30:0] od;
               wire [19:0] d;
               wire [11:0] q;
               assign o = od[10:0];
               rcstream_filter_input c0(.clock_reset(clock_reset), .i(d[10:0]), .o(q[10:0]));
               rcstream_filter_pred c1(.clock_reset(clock_reset), .i(d[19:11]), .o(q[11:11]));
               assign d = od[30:11];
               assign od = kernel_filter_kernel(clock_reset, i, q);
               function [30:0] kernel_filter_kernel(input reg [1:0] arg_0, input reg [10:0] arg_1, input reg [11:0] arg_2);
                     reg [9:0] r0;
                     reg [10:0] r1;
                     // d
                     reg [19:0] r2;
                     reg [10:0] r3;
                     reg [11:0] r4;
                     reg [9:0] r5;
                     reg [0:0] r6;
                     reg [8:0] r7;
                     reg [9:0] r8;
                     reg [9:0] r9;
                     reg [0:0] r10;
                     reg [8:0] r11;
                     // d
                     reg [19:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     reg [0:0] r16;
                     reg [0:0] r17;
                     reg [0:0] r18;
                     reg [0:0] r19;
                     // d
                     reg [19:0] r20;
                     reg [9:0] r21;
                     reg [8:0] r22;
                     reg [9:0] r23;
                     reg [10:0] r24;
                     reg [0:0] r25;
                     reg [10:0] r26;
                     reg [10:0] r27;
                     reg [30:0] r28;
                     reg [1:0] r29;
                     localparam l0 = 20'bXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 10'bXXXXXXXXX0;
                     localparam l5 = 1'b1;
                     localparam l6 = 10'b0000000000;
                     localparam l7 = 11'b00000000000;
                     begin
                        r29 = arg_0;
                        r1 = arg_1;
                        r4 = arg_2;
                        r0 = r1[9:0];
                        r2 = l0;
                        r2[9:0] = r0;
                        r3 = r4[10:0];
                        r5 = r3[9:0];
                        r6 = r5[9:9];
                        r7 = r5[8:0];
                        r8 = {r7, l1};
                        case (r6)
                           1'b1 : r9 = r8;
                           1'b0 : r9 = l4;
                        endcase
                        r10 = r9[0:0];
                        r11 = r9[9:1];
                        r12 = r2;
                        r12[19:11] = r11;
                        r13 = r4[11:11];
                        r14 = r10 & r13;
                        r15 = r4[11:11];
                        r16 = ~r15;
                        r17 = r10 & r16;
                        r18 = r1[10:10];
                        r19 = r18 | r17;
                        r20 = r12;
                        r20[10:10] = r19;
                        r22 = r11[8:0];
                        r21 = {l5, r22};
                        r23 = r14 ? r21 : l6;
                        r24 = r4[10:0];
                        r25 = r24[10:10];
                        r26 = l7;
                        r26[9:0] = r23;
                        r27 = r26;
                        r27[10:10] = r25;
                        r28 = {r20, r27};
                        kernel_filter_kernel = r28;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip on both RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = RCStreamFilter::<b8, bool>::try_new::<keep_even>()?;
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = RCStreamFilter::<b8, bool>::try_new::<keep_even>()?;
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_filter");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "7b2cdeee89dab747e4c77de84e6e01f05310aa05596bcadcb67bed40335cc407"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_filter.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
