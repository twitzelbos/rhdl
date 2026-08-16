#![warn(missing_docs)]
//! Transform and drop items on an [`RCStream`] in one pass.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +RCStreamFilterMap-+
 ?T   |                  |  ?S
+---->| data        data +---->
      |                  |
<-----+ ready      ready |<----+
      |                  |
      +------------------+
")]
//!
//!# Framing
//!
//! The function is `fn(ClockReset, Item<T, F>) -> Option<Item<S, F>>`
//! — it sees the whole incoming [`Item`] and produces a whole outgoing
//! one, so it can rewrite the framing marker as well as the payload.
//!
//! It sees the input's framing marker for the same reason
//! [`super::filter`] does: returning `None` **drops** the item, and
//! dropping the item that carries an end-of-frame marker means the
//! frame never ends.  Making `F` visible to the function is what lets
//! the author notice and handle it.  See the `filter` module docs for
//! the full argument.
//!
//! Because the function also *constructs* the output item, it can
//! relocate a marker rather than merely preserve it — e.g. drop a
//! payload but move its `frame` onto the next emitted item.
//!
//!# Dropped items advance the buffer
//!
//! As in [`super::filter`], an item the function rejects is consumed
//! from the internal skid buffer regardless of the downstream `ready`:
//!
//! ```text
//! d.input.ready = i.ready || dropping
//! ```
//!
//! A sink is permitted to gate its `ready` on `data.is_some()`, and a
//! dropped item shows it no data, so waiting for downstream would
//! deadlock.
//!
//!# Internals
//!
//! An [`RCStreamRelay`] (Carloni skid buffer) feeding a combinational
//! [`Func`].  No combinational path from any input to any output.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_filter_map.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_filter_map.md")]

use rhdl::{
    core::{ClockReset, DigitalFn, DigitalFn2, RHDLError},
    prelude::*,
};

use super::bus::{Item, RCStream};
use super::relay::RCStreamRelay;

/// Transform and drop items on an [`RCStream`] in one pass.
///
/// `T` is the input payload type, `S` the output payload type, `F` the
/// framing-marker type.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamFilterMap<T: Digital, F: Digital, S: Digital> {
    /// Carloni skid buffer isolating the upstream handshake.
    input: RCStreamRelay<T, F>,
    /// The combinational transform, over whole items.
    func: Func<Item<T, F>, Option<Item<S, F>>>,
}

impl<T: Digital, F: Digital, S: Digital> RCStreamFilterMap<T, F, S> {
    /// Construct an [`RCStreamFilterMap`] from a synthesizable function.
    ///
    /// `K` must be a `#[kernel]` function with the signature
    /// `fn(ClockReset, Item<T, F>) -> Option<Item<S, F>>`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = Item<T, F>, O = Option<Item<S, F>>>,
    {
        Ok(Self {
            input: RCStreamRelay::default(),
            func: Func::try_new::<K>()?,
        })
    }
}

impl<T: Digital, F: Digital, S: Digital> SynchronousIO for RCStreamFilterMap<T, F, S> {
    type I = RCStream<T, F>;
    type O = RCStream<S, F>;
    type Kernel = filter_map_kernel<T, F, S>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn filter_map_kernel<T: Digital, F: Digital, S: Digital>(
    _cr: ClockReset,
    i: RCStream<T, F>,
    q: Q<T, F, S>,
) -> (RCStream<S, F>, D<T, F, S>) {
    let mut d = D::<T, F, S>::dont_care();
    d.input.data = i.data;

    // Present the buffered item (if any) to the function.
    let (have, item) = match q.input.data {
        Some(it) => (true, it),
        None => (false, Item::<T, F>::dont_care()),
    };
    d.func = item;

    // The function's verdict is only meaningful when we actually had an
    // item to give it.
    let (produced, out_item) = match q.func {
        Some(s) => (true, s),
        None => (false, Item::<S, F>::dont_care()),
    };
    let emit = have && produced;
    let dropping = have && !produced;

    // Consume rejected items ourselves — see the module docs.
    d.input.ready = i.ready || dropping;

    let o = RCStream::<S, F> {
        data: if emit { Some(out_item) } else { None },
        ready: q.input.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::slice::lsbs;
    use rhdl::core::sim::ResetOrData;

    /// Halve even payloads, drop odd ones, and never drop a
    /// marker-carrying item (relocating framing is the point of the
    /// `Item -> Item` signature).
    #[kernel]
    fn halve_even(_cr: ClockReset, it: Item<b8, bool>) -> Option<Item<b4, bool>> {
        if it.frame || ((it.data & bits::<8>(1)) == bits::<8>(0)) {
            Some(Item::<b4, bool> {
                data: lsbs::<4, 8>(it.data >> 1),
                frame: it.frame,
            })
        } else {
            None
        }
    }

    fn item(v: u128, frame: bool) -> Item<b8, bool> {
        Item::<b8, bool> {
            data: bits::<8>(v),
            frame,
        }
    }

    fn q_with(
        data: Option<Item<b8, bool>>,
        func: Option<Item<b4, bool>>,
        ready: bool,
    ) -> Q<b8, bool, b4> {
        Q::<b8, bool, b4> {
            input: RCStream::<b8, bool> { data, ready },
            func,
        }
    }

    /// Tier 1 — a produced item is emitted, carrying whatever framing
    /// the function chose.
    #[test]
    fn produced_item_is_emitted() {
        let out = Item::<b4, bool> {
            data: bits::<4>(0x5),
            frame: true,
        };
        let q = q_with(Some(item(0x0A, true)), Some(out), true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, d) = filter_map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        match o.data {
            Some(it) => {
                assert_eq!(it.data.raw(), 0x5);
                assert!(it.frame);
            }
            None => panic!("expected an emitted item"),
        }
        assert_eq!(d.func.data.raw(), 0x0A, "function sees the whole item");
    }

    /// Tier 1 — `None` from the function drops the item.
    #[test]
    fn none_drops_the_item() {
        let q = q_with(Some(item(0x0B, false)), None, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, _d) = filter_map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
    }

    /// Tier 1 — the deadlock guard: a dropped item is consumed even
    /// with the sink stalled.
    #[test]
    fn dropped_item_advances_buffer_without_downstream_ready() {
        let q = q_with(Some(item(0x0B, false)), None, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (_o, d) = filter_map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(
            d.input.ready,
            "a dropped item must be consumed regardless of downstream ready"
        );
    }

    /// Tier 1 — an emitted item is held while the sink stalls.
    #[test]
    fn emitted_item_is_held_while_downstream_stalls() {
        let out = Item::<b4, bool> {
            data: bits::<4>(0x5),
            frame: false,
        };
        let q = q_with(Some(item(0x0A, false)), Some(out), true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (o, d) = filter_map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_some());
        assert!(!d.input.ready, "an emitted item must not be discarded");
    }

    /// Tier 1 — with an empty buffer, a stale `Some` from the function
    /// must NOT be emitted.  `q.func` is combinational garbage when it
    /// was handed a don't-care item, so the `have` guard is what keeps
    /// a phantom item off the bus.
    #[test]
    fn empty_buffer_never_emits_even_if_func_says_some() {
        let stale = Item::<b4, bool> {
            data: bits::<4>(0xF),
            frame: true,
        };
        let q = q_with(None, Some(stale), true);
        // Downstream held off, so the only thing that could assert
        // `d.input.ready` is a spurious drop.
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (o, d) = filter_map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(
            o.data.is_none(),
            "an empty buffer must not emit, whatever the function returns"
        );
        assert!(
            !d.input.ready,
            "an empty buffer must not manufacture a drop pulse"
        );
    }

    /// Tier 1 — the complement of the above: with nothing buffered and
    /// the sink ready, `ready` propagates so the relay can accept new
    /// data.  (Withholding it here would stall the pipeline.)
    #[test]
    fn empty_buffer_propagates_ready_upstream() {
        let q = q_with(None, None, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (_o, d) = filter_map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(
            d.input.ready,
            "an idle stage must pass ready upstream or the pipeline stalls"
        );
    }

    /// The LID requirement: no combinational path from any input to any
    /// output.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = RCStreamFilterMap::<b8, bool, b4>::try_new::<halve_even>()?;
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed-loop end-to-end against a data-gated sink (the
    /// case that would deadlock a naive implementation), checking both
    /// halves of the operation: the transform and the drop.
    #[test]
    fn stream_transforms_and_drops_in_order() -> Result<(), RHDLError> {
        const COUNT: u128 = 40;
        let uut = RCStreamFilterMap::<b8, bool, b4>::try_new::<halve_even>()?;
        let mut to_send: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
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

        // Kept when even or frame-marked; payload halved, then truncated
        // to 4 bits by the kernel's `lsbs`.
        let expect: Vec<u128> = (0..COUNT)
            .filter(|k| k % 8 == 7 || k % 2 == 0)
            .map(|k| (k >> 1) & 0xF)
            .collect();
        assert_eq!(got, expect, "transform and drop must both be exact");
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
        let uut = RCStreamFilterMap::<b8, bool, b4>::try_new::<halve_even>()?;
        let desc = uut.descriptor("rcstream_filter_map".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_filter_map")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_filter_map(input wire [1:0] clock_reset, input wire [10:0] i, output wire [6:0] o);
               wire [26:0] od;
               wire [19:0] d;
               wire [16:0] q;
               assign o = od[6:0];
               rcstream_filter_map_input c0(.clock_reset(clock_reset), .i(d[10:0]), .o(q[10:0]));
               rcstream_filter_map_func c1(.clock_reset(clock_reset), .i(d[19:11]), .o(q[16:11]));
               assign d = od[26:7];
               assign od = kernel_filter_map_kernel(clock_reset, i, q);
               function [26:0] kernel_filter_map_kernel(input reg [1:0] arg_0, input reg [10:0] arg_1, input reg [16:0] arg_2);
                     reg [9:0] r0;
                     reg [10:0] r1;
                     // d
                     reg [19:0] r2;
                     reg [10:0] r3;
                     reg [16:0] r4;
                     reg [9:0] r5;
                     reg [0:0] r6;
                     reg [8:0] r7;
                     reg [9:0] r8;
                     reg [9:0] r9;
                     reg [0:0] r10;
                     reg [8:0] r11;
                     // d
                     reg [19:0] r12;
                     reg [5:0] r13;
                     reg [0:0] r14;
                     reg [4:0] r15;
                     reg [5:0] r16;
                     reg [5:0] r17;
                     reg [0:0] r18;
                     reg [4:0] r19;
                     reg [0:0] r20;
                     reg [0:0] r21;
                     reg [0:0] r22;
                     reg [0:0] r23;
                     reg [0:0] r24;
                     // d
                     reg [19:0] r25;
                     reg [5:0] r26;
                     reg [4:0] r27;
                     reg [5:0] r28;
                     reg [10:0] r29;
                     reg [0:0] r30;
                     reg [6:0] r31;
                     reg [6:0] r32;
                     reg [26:0] r33;
                     reg [1:0] r34;
                     localparam l0 = 20'bXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 10'bXXXXXXXXX0;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 6'bXXXXX0;
                     localparam l9 = 1'b1;
                     localparam l10 = 6'b000000;
                     localparam l11 = 7'b0000000;
                     begin
                        r34 = arg_0;
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
                        r13 = r4[16:11];
                        r14 = r13[5:5];
                        r15 = r13[4:0];
                        r16 = {r15, l5};
                        case (r14)
                           1'b1 : r17 = r16;
                           1'b0 : r17 = l8;
                        endcase
                        r18 = r17[0:0];
                        r19 = r17[5:1];
                        r20 = r10 & r18;
                        r21 = ~r18;
                        r22 = r10 & r21;
                        r23 = r1[10:10];
                        r24 = r23 | r22;
                        r25 = r12;
                        r25[10:10] = r24;
                        r27 = r19[4:0];
                        r26 = {l9, r27};
                        r28 = r20 ? r26 : l10;
                        r29 = r4[10:0];
                        r30 = r29[10:10];
                        r31 = l11;
                        r31[5:0] = r28;
                        r32 = r31;
                        r32[6:6] = r30;
                        r33 = {r25, r32};
                        kernel_filter_map_kernel = r33;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip on both RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = RCStreamFilterMap::<b8, bool, b4>::try_new::<halve_even>()?;
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = RCStreamFilterMap::<b8, bool, b4>::try_new::<halve_even>()?;
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_filter_map");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "48e0007339492882ac9e2709c4bfafe6a0a882691cd2c882967b5671e792a19b"
        ];
        let digest = vcd
            .dump_to_file(root.join("rcstream_filter_map.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
