//! Filter Map Stream Core
//!
//!# Purpose
//!
//! A [FilterMap] Core takes a sequence of elements of type `T`
//! and a function `fn(T) -> Option<S>`, and keeps only those
//! items which are `Some`.  This is particularly handy for
//! processing streams of `enum` values, and then extracting
//! a particular variant, for example.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [FilterMap] stream
//!
#![doc = badascii_formal!("
     ++FilterMap+---+        
 ?T  |              | ?S    
+--->+ data   data  +---->
 R<T>|              | R<S>       
<----+ ready  ready |<---+
     +--------------+       
")]
//!
//!# Internals
//!
//! Unlike [FlattenPipe] or [ChunkedPipe], the [FilterMap] does not
//! impose any flow control on the upstream pipe.  Because it can
//! at most produce as many items as the source pipe, it can be
//! implemented with simple [StreamBuffer] buffers at the input
//! and output, which are needed to isolate the combinatorial
//! filter-map function from the remaining parts of the pipeline.  
//! Note that if you need a more expensive filter-map function (i.e., one
//! that itself is pipelined), then you cannot use this construct.
//!
#![doc = badascii!(r"
                                    ++func++   +          
                                    |      |?S |\         
                                  +>|in out+-->|1+  data  
     +-+Input Buf++     +unpack+  | +------+   | +------->
 ?T  |            | ?T  |      |T |     None+->|0+        
+--->|data    data+---->|in out+--+            |/         
 R<T>|            |     |      |               +^   R<S> 
<----+ready  ready|<-+  |   tag+----------------+  +-----+
     +------------+  |  +------+                   |      
                     +-----------------------------+      
")]
//!# Example
//!
//! Here is an example of running the pipeline filter map.  It is
//! interesting, because it demonstrates the use of `enum` values
//! to thar are filtered and the payload stripped for further
//! processing.
//!
//!```
#![doc = include_str!("../../examples/filter_map.rs")]
//!```
//!
//! with a trace file like this:
//!
#![doc = include_str!("../../doc/filter_map.md")]
//!

use badascii_doc::{badascii, badascii_formal};

use rhdl::prelude::*;

use crate::stream::ready;

use super::{StreamIO, stream_buffer::StreamBuffer};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The FilterMap Core
///
/// Here `T` is the input type, and `S` is the
/// output type.  A provided (combinatorial) function
/// performs the mapping function.  It must have a
/// signature of `fn(T) -> Option<S>`.
pub struct FilterMap<T: Digital, S: Digital> {
    input_buffer: StreamBuffer<T>,
    func: Func<T, Option<S>>,
}

impl<T, S> FilterMap<T, S>
where
    T: Digital,
    S: Digital,
{
    /// Construct a Filter Map Stream
    ///
    /// The argument to the filter map
    /// `try_new` function is a synthesizable function
    /// (i.e., one marked with the `#[kernel]` attribute).
    /// It must have a signature `fn(T) -> Option<S>`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = T, O = Option<S>>,
    {
        Ok(Self {
            input_buffer: StreamBuffer::default(),
            func: Func::try_new::<K>()?,
        })
    }
}

/// The input for the [FilterMap]
pub type In<T, S> = StreamIO<T, S>;

/// The output type for the [FilterMap]
pub type Out<T, S> = StreamIO<S, T>;

impl<T, S> SynchronousIO for FilterMap<T, S>
where
    T: Digital,
    S: Digital,
{
    type I = In<T, S>;
    type O = Out<T, S>;
    type Kernel = kernel<T, S>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn kernel<T, S>(_cr: ClockReset, i: In<T, S>, q: Q<T, S>) -> (Out<T, S>, D<T, S>)
where
    T: Digital,
    S: Digital,
{
    let mut d = D::<T, S>::dont_care();
    d.input_buffer.data = i.data;
    d.func = T::dont_care();
    let mut have = false;
    if let Some(data) = q.input_buffer.data {
        d.func = data;
        have = true;
    }
    // The function's verdict only means anything when we had an item to
    // hand it.
    let produced = match q.func {
        Some(_) => true,
        None => false,
    };
    let emit = have && produced;
    // A DROPPED item must be consumed by us, not by the sink — see
    // `super::filter` for the full argument.  A sink may gate its
    // `ready` on seeing data, and a dropped item shows it none, so
    // waiting for `i.ready` here deadlocks the stream.
    let dropping = have && !produced;
    d.input_buffer.ready = ready::<T>(i.ready.raw || dropping);
    let o = Out::<T, S> {
        data: if emit { q.func } else { None },
        ready: q.input_buffer.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use crate::{
        core::slice::lsbs,
        rng::xorshift::XorShift128,
        stream::testing::{single_stage::single_stage, utils::stalling},
    };

    use super::*;
    use crate::stream::testing::sink_from_fn::SinkView;

    #[kernel]
    fn filter_map_item(_cr: ClockReset, t: b4) -> Option<b2> {
        if (t & bits(1)).any() {
            None
        } else {
            Some(lsbs::<2, 4>(t))
        }
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let map = FilterMap::try_new::<filter_map_item>()?;
        drc::no_combinatorial_paths(&map)?;
        Ok(())
    }

    #[kernel]
    fn halve_even_regression(_cr: ClockReset, t: b4) -> Option<b4> {
        if !(t & bits(1)).any() {
            Some(t >> 1)
        } else {
            None
        }
    }

    /// **Regression: deadlock against a data-gated sink.**
    ///
    /// The AXI Ready/Valid contract this module implements permits a
    /// sink to withhold `ready` until it sees data. A dropped item
    /// produces `data = None` downstream, so such a sink never asserts
    /// `ready` — and this widget used to gate its input buffer on
    /// `i.ready` alone, leaving the dropped item stuck forever. The
    /// stream deadlocked after the first one, with everything behind it
    /// silently lost.
    ///
    /// The pre-existing `test_operation` missed it twice over: its sink
    /// returns `rand::random::<f64>() > 0.2`, which is independent of
    /// whether data was presented, and it only asserts a property of the
    /// values that *do* arrive rather than that all of them do.
    #[test]
    fn no_deadlock_against_a_data_gated_sink() -> Result<(), RHDLError> {
        use crate::stream::testing::closed_loop::assert_lossless_mapped;

        const COUNT: u128 = 16;
        let uut = FilterMap::<b4, b4>::try_new::<halve_even_regression>()?;
        let src: Vec<b4> = (0..COUNT).map(|k| b4(k % 16)).collect();
        // Survivors only, halved — see `filter` for why a subsequence is
        // still a whole-sequence comparison here.
        let want: Vec<b4> = (0..COUNT)
            .filter(|k| k % 2 == 0)
            .map(|k| b4(k >> 1))
            .collect();
        assert_lossless_mapped(&uut, &src, &want);
        Ok(())
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// Gaps the offered data on one cadence and withholds `ready` on
    /// another, coprime to it. As with `filter`, the rejected-item path
    /// is the one that needs this: a rejected item presents nothing
    /// downstream, so it cannot depend on the sink asserting `ready` to
    /// make progress.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4, b2>)>> {
        (0..24u128)
            .map(|k| In::<b4, b2> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b4(k % 16))
                },
                ready: ready::<b2>(!k.is_multiple_of(3)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = FilterMap::<b4, b2>::try_new::<filter_map_item>()?;
        let desc = uut.descriptor("stream_filter_map".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_filter_map")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_filter_map(input wire [1:0] clock_reset, input wire [5:0] i, output wire [3:0] o);
               wire [13:0] od;
               wire [9:0] d;
               wire [8:0] q;
               assign o = od[3:0];
               stream_filter_map_input_buffer c0(.clock_reset(clock_reset), .i(d[5:0]), .o(q[5:0]));
               stream_filter_map_func c1(.clock_reset(clock_reset), .i(d[9:6]), .o(q[8:6]));
               assign d = od[13:4];
               assign od = kernel_kernel(clock_reset, i, q);
               function [13:0] kernel_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [8:0] arg_2);
                     reg [4:0] r0;
                     reg [5:0] r1;
                     // d
                     reg [9:0] r2;
                     // d
                     reg [9:0] r3;
                     reg [5:0] r4;
                     reg [8:0] r5;
                     reg [4:0] r6;
                     reg [0:0] r7;
                     reg [3:0] r8;
                     // d
                     reg [9:0] r9;
                     // d
                     reg [9:0] r10;
                     // have
                     reg [0:0] r11;
                     reg [2:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     reg [0:0] r16;
                     reg [0:0] r17;
                     reg [0:0] r18;
                     reg [0:0] r19;
                     reg [0:0] r20;
                     reg [0:0] r21;
                     // d
                     reg [9:0] r22;
                     reg [2:0] r23;
                     reg [2:0] r24;
                     reg [5:0] r25;
                     reg [0:0] r26;
                     reg [3:0] r27;
                     reg [3:0] r28;
                     reg [13:0] r29;
                     reg [1:0] r30;
                     localparam l0 = 10'bXXXXXXXXXX;
                     localparam l1 = 4'bXXXX;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b0;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 1'b0;
                     localparam l9 = 1'b0;
                     localparam l10 = 3'b000;
                     localparam l11 = 4'b0000;
                     begin
                        r30 = arg_0;
                        r1 = arg_1;
                        r5 = arg_2;
                        r0 = r1[4:0];
                        r2 = l0;
                        r2[4:0] = r0;
                        r3 = r2;
                        r3[9:6] = l1;
                        r4 = r5[5:0];
                        r6 = r4[4:0];
                        r7 = r6[4:4];
                        r8 = r6[3:0];
                        r9 = r3;
                        r9[9:6] = r8;
                        case (r7)
                           1'b1 : r10 = r9;
                           default : r10 = r3;
                        endcase
                        case (r7)
                           1'b1 : r11 = l3;
                           default : r11 = l4;
                        endcase
                        r12 = r5[8:6];
                        r13 = r12[2:2];
                        case (r13)
                           1'b1 : r14 = l6;
                           1'b0 : r14 = l8;
                        endcase
                        r15 = r11 & r14;
                        r16 = ~r14;
                        r17 = r11 & r16;
                        r18 = r1[5:5];
                        r19 = r18 | r17;
                        r20 = l9;
                        r21 = r20;
                        r21[0:0] = r19;
                        r22 = r10;
                        r22[5:5] = r21;
                        r23 = r5[8:6];
                        r24 = r15 ? r23 : l10;
                        r25 = r5[5:0];
                        r26 = r25[5:5];
                        r27 = l11;
                        r27[2:0] = r24;
                        r28 = r27;
                        r28[3:3] = r26;
                        r29 = {r22, r28};
                        kernel_kernel = r29;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = FilterMap::<b4, b2>::try_new::<filter_map_item>()?;
        let tb = uut
            .run(bench_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = FilterMap::<b4, b2>::try_new::<filter_map_item>()?;
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_filter_map");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "2bf878af81a348a0e5a2020fdc85ee2afc231bb97de3a7924f587fe28b362c13"
        ];
        let digest = vcd
            .dump_to_file(root.join("stream_filter_map.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let mut b_rng = a_rng.clone().filter_map(|x| {
            if (x & bits(1)).any() {
                None
            } else {
                Some(lsbs::<2, 4>(x))
            }
        });
        let a_rng = stalling(a_rng, 0.23);
        let consume = move |v: SinkView<b2>| {
            if let Some(data) = v.accepted {
                let orig = b_rng.next().unwrap();
                assert_eq!(data, orig);
            }
            rand::random::<f64>() > 0.2
        };
        let map = FilterMap::try_new::<filter_map_item>()?;
        let uut = single_stage(map, a_rng, consume);
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
