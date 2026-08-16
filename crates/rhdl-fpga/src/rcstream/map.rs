#![warn(missing_docs)]
//! Apply a combinational function to every payload on an [`RCStream`].
//!
//! [`RCStreamMap`] transforms the payload `T` into `S` and passes the
//! framing marker `F` through untouched.  It is the `rcstream`
//! counterpart to [`crate::stream::map`].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+RCStreamMap+----+
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
//! The framing marker rides through unchanged: an input
//! `Item { data, frame }` becomes `Item { data: f(data), frame }`.
//! That is why the mapped function takes the **payload only**
//! (`fn(ClockReset, T) -> S`) rather than the whole [`Item`] — a
//! payload transformation is orthogonal to framing, so preserving `F`
//! is unambiguously the right behaviour and there is nothing for the
//! user to decide.
//!
//! This is deliberately *not* how [`super::filter`] and
//! [`super::filter_map`] work.  Those can **drop** items, and dropping
//! an item destroys whatever framing information it carried — drop the
//! item holding the end-of-frame marker and the frame never ends.  So
//! they hand the predicate the whole `Item` to make that hazard
//! visible.  `map` cannot drop anything, so the question never arises.
//!
//!# Internals
//!
//! An [`RCStreamRelay`] (Carloni skid buffer) in front of a
//! combinational [`Func`].  The relay is what keeps the widget
//! latency-insensitive and guarantees no combinational path from any
//! input to any output — verified by a
//! [`drc::no_combinatorial_paths`] test.
//!
#![doc = badascii_doc::badascii!("
   data   +---------+  item   +------+  f(data)
  +------>| Relay   +-------->| Func +--------->
          | (skid)  |         +------+
  <-------+ ready   |<--------------------------+
   ready  +---------+            ready
")]
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_map.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_map.md")]

use rhdl::{
    core::{ClockReset, DigitalFn, DigitalFn2, RHDLError},
    prelude::*,
};

use super::bus::{Item, RCStream};
use super::relay::RCStreamRelay;

/// Apply a combinational function to every payload on an [`RCStream`].
///
/// `T` is the input payload type, `S` the output payload type, and `F`
/// the framing-marker type, which is preserved.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamMap<T: Digital, F: Digital, S: Digital> {
    /// Carloni skid buffer isolating the upstream handshake.
    input: RCStreamRelay<T, F>,
    /// The combinational mapping function.
    func: Func<T, S>,
}

impl<T: Digital, F: Digital, S: Digital> RCStreamMap<T, F, S> {
    /// Construct an [`RCStreamMap`] from a synthesizable function.
    ///
    /// `K` must be a `#[kernel]` function with the signature
    /// `fn(ClockReset, T) -> S`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = T, O = S>,
    {
        Ok(Self {
            input: RCStreamRelay::default(),
            func: Func::try_new::<K>()?,
        })
    }
}

impl<T: Digital, F: Digital, S: Digital> SynchronousIO for RCStreamMap<T, F, S> {
    type I = RCStream<T, F>;
    type O = RCStream<S, F>;
    type Kernel = map_kernel<T, F, S>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn map_kernel<T: Digital, F: Digital, S: Digital>(
    _cr: ClockReset,
    i: RCStream<T, F>,
    q: Q<T, F, S>,
) -> (RCStream<S, F>, D<T, F, S>) {
    let mut d = D::<T, F, S>::dont_care();

    // Upstream data into the skid buffer; downstream ready back into it.
    d.input.data = i.data;
    d.input.ready = i.ready;

    // Apply the function to whatever the buffer is presenting, keeping
    // the framing marker attached to that same item.
    let o_data = match q.input.data {
        Some(it) => {
            d.func = it.data;
            Some(Item::<S, F> {
                data: q.func,
                frame: it.frame,
            })
        }
        None => {
            d.func = T::dont_care();
            None
        }
    };

    let o = RCStream::<S, F> {
        data: o_data,
        ready: q.input.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::slice::lsbs;
    use rhdl::core::sim::ResetOrData;

    #[kernel]
    fn narrow(_cr: ClockReset, t: b8) -> b4 {
        lsbs::<4, 8>(t)
    }

    fn item(v: u128, frame: bool) -> Item<b8, bool> {
        Item::<b8, bool> {
            data: bits::<8>(v),
            frame,
        }
    }

    /// Tier 1 — an item in the buffer is mapped, and its framing marker
    /// rides through untouched.
    #[test]
    fn maps_payload_and_preserves_frame() -> Result<(), RHDLError> {
        let _uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
        let q = Q::<b8, bool, b4> {
            input: RCStream::<b8, bool> {
                data: Some(item(0xAB, true)),
                ready: true,
            },
            func: bits::<4>(0xB),
        };
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, d) = map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        // The payload handed to the function is the buffered item's data.
        assert_eq!(d.func.raw(), 0xAB);
        match o.data {
            Some(it) => {
                assert_eq!(it.data.raw(), 0xB, "payload must be the function's output");
                assert!(it.frame, "framing marker must survive the map");
            }
            None => panic!("expected a mapped item"),
        }
        Ok(())
    }

    /// Tier 1 — an empty buffer produces an idle output.
    #[test]
    fn idle_buffer_produces_idle_output() -> Result<(), RHDLError> {
        let q = Q::<b8, bool, b4> {
            input: RCStream::<b8, bool> {
                data: None,
                ready: false,
            },
            func: bits::<4>(0),
        };
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, _d) = map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
        assert!(!o.ready, "ready must mirror the buffer's ready");
        Ok(())
    }

    /// Tier 1 — the handshake is wired straight through: upstream data
    /// into the buffer, downstream ready back into it, buffer's ready
    /// out to upstream.
    #[test]
    fn handshake_is_wired_through() -> Result<(), RHDLError> {
        let q = Q::<b8, bool, b4> {
            input: RCStream::<b8, bool> {
                data: None,
                ready: true,
            },
            func: bits::<4>(0),
        };
        let i = RCStream::<b8, bool> {
            data: Some(item(0x12, false)),
            ready: false,
        };
        let (o, d) = map_kernel::<b8, bool, b4>(ClockReset::dont_care(), i, q);
        assert!(
            d.input.data.is_some(),
            "upstream data must reach the buffer"
        );
        assert!(!d.input.ready, "downstream ready must reach the buffer");
        assert!(o.ready, "buffer's ready must reach upstream");
        Ok(())
    }

    /// The LID requirement: no combinational path from any input to any
    /// output.  This is what makes relay insertion sound around this
    /// widget, and it is why the skid buffer is not optional.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed-loop end-to-end.  Every item offered must come
    /// out exactly once, in order, mapped, with its framing intact,
    /// under deterministic sink backpressure.
    #[test]
    fn stream_maps_every_item_in_order() -> Result<(), RHDLError> {
        const COUNT: u128 = 32;
        let uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
        let mut to_send: u128 = 0;
        let mut got: Vec<(u128, bool)> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = !phase.is_multiple_of(3);
                if let Some(it) = output.data {
                    if sink_ready {
                        got.push((it.data.raw(), it.frame));
                    }
                }
                let mut input = RCStream::<b8, bool> {
                    data: None,
                    ready: sink_ready,
                };
                if to_send < COUNT && output.ready {
                    // frame marker on every 8th item
                    input.data = Some(item(to_send, to_send % 8 == 7));
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 100_000)
        .for_each(drop);

        let expect: Vec<(u128, bool)> = (0..COUNT).map(|k| (k & 0xF, k % 8 == 7)).collect();
        assert_eq!(
            got, expect,
            "every item must emerge exactly once, in order, mapped, framing preserved"
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
        let uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
        let desc = uut.descriptor("rcstream_map".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_map")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_map(input wire [1:0] clock_reset, input wire [10:0] i, output wire [6:0] o);
               wire [25:0] od;
               wire [18:0] d;
               wire [14:0] q;
               assign o = od[6:0];
               rcstream_map_input c0(.clock_reset(clock_reset), .i(d[10:0]), .o(q[10:0]));
               rcstream_map_func c1(.clock_reset(clock_reset), .i(d[18:11]), .o(q[14:11]));
               assign d = od[25:7];
               assign od = kernel_map_kernel(clock_reset, i, q);
               function [25:0] kernel_map_kernel(input reg [1:0] arg_0, input reg [10:0] arg_1, input reg [14:0] arg_2);
                     reg [9:0] r0;
                     reg [10:0] r1;
                     // d
                     reg [18:0] r2;
                     reg [0:0] r3;
                     // d
                     reg [18:0] r4;
                     reg [10:0] r5;
                     reg [14:0] r6;
                     reg [9:0] r7;
                     reg [0:0] r8;
                     reg [8:0] r9;
                     reg [7:0] r10;
                     // d
                     reg [18:0] r11;
                     reg [3:0] r12;
                     reg [0:0] r13;
                     reg [4:0] r14;
                     reg [4:0] r15;
                     reg [5:0] r16;
                     reg [4:0] r17;
                     // d
                     reg [18:0] r18;
                     // d
                     reg [18:0] r19;
                     reg [5:0] r20;
                     reg [10:0] r21;
                     reg [0:0] r22;
                     reg [6:0] r23;
                     reg [6:0] r24;
                     reg [25:0] r25;
                     reg [1:0] r26;
                     localparam l0 = 19'bXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 5'b00000;
                     localparam l2 = 1'b1;
                     localparam l3 = 8'bXXXXXXXX;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b0;
                     localparam l6 = 6'b000000;
                     localparam l7 = 7'b0000000;
                     begin
                        r26 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[9:0];
                        r2 = l0;
                        r2[9:0] = r0;
                        r3 = r1[10:10];
                        r4 = r2;
                        r4[10:10] = r3;
                        r5 = r6[10:0];
                        r7 = r5[9:0];
                        r8 = r7[9:9];
                        r9 = r7[8:0];
                        r10 = r9[7:0];
                        r11 = r4;
                        r11[18:11] = r10;
                        r12 = r6[14:11];
                        r13 = r9[8:8];
                        r14 = l1;
                        r14[3:0] = r12;
                        r15 = r14;
                        r15[4:4] = r13;
                        r17 = r15[4:0];
                        r16 = {l2, r17};
                        r18 = r4;
                        r18[18:11] = l3;
                        case (r8)
                           1'b1 : r19 = r11;
                           1'b0 : r19 = r18;
                        endcase
                        case (r8)
                           1'b1 : r20 = r16;
                           1'b0 : r20 = l6;
                        endcase
                        r21 = r6[10:0];
                        r22 = r21[10:10];
                        r23 = l7;
                        r23[5:0] = r20;
                        r24 = r23;
                        r24[6:6] = r22;
                        r25 = {r19, r24};
                        kernel_map_kernel = r25;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip on both RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = RCStreamMap::<b8, bool, b4>::try_new::<narrow>()?;
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_map");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "7bd0c8f8f8e4f9f1d5a4cfde984036681162e308aa93809c8c882a54e0d6393d"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_map.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
