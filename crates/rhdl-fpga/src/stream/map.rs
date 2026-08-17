//! Map Stream Core
//!
//!# Purpose
//!
//! A [Map] Core takes a stream of elements of type `T` and
//! a synthesizable function `fn(T) -> S`, and feeds a stream
//! that carries type `S`.  This is equivalent to using `.map()` on
//! an interator.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Map] buffer
//!
#![doc = badascii_formal!("
     +--+Map+---------+        
 ?T  |                | ?S   
+--->+ data     data  +---->
Ry<T>|                | Ry<S>       
<----+ ready    ready |<---+
     +----------------+       
")]
//!
//!# Internals
//!
//! Unlike [Flatten] or [Chunked], the [Map] does not
//! impose any flow control on the upstream.  Because it can
//! at most produce as many items as the source stream, it can be
//! implemented with simple [StreamBuffer] buffers at the input
//! and output, which are needed to isolate the combinatorial
//! `map` function from the remaining parts of the stream.  
//! Note that if you need a more expensive `map` function (i.e., one
//! that itself is pipelined), then you cannot use this construct.
//!
#![doc = badascii!(r"
                                      +-+Func+--+                       
                                      |         | S                     
                                    +>|in    out+--+                    
     +-+Buffer+---+     +-+upck+-+  | +---------+  |   +-+pck+-+        
 ?T  |            | ?T  |        |T |              |   |       |   ?S
+--->|data    data+---->|in   out+--+              +-->|in  out+------->
Ry<T>|            |     |        |                     |       |   Ry<S>
<----+ready  ready|<-+  |     tag+-------------------->|tag    |  +----+
     +------------+  |  +--------+                     +-------+  |     
                     |                                            |     
                     +--------------------------------------------+     
")]
//!# Example
//!
//! Here is an example of mapping a stream, transforming
//! elements.
//!
//!```
#![doc = include_str!("../../examples/stream_map.rs")]
//!```
//!
//! with a trace file like this:
//!
#![doc = include_str!("../../doc/stream_map.md")]
//!

use badascii_doc::{badascii, badascii_formal};
use rhdl::{
    core::{ClockReset, DigitalFn, DigitalFn2, RHDLError},
    prelude::*,
};

use super::{StreamIO, ready_cast, stream_buffer::StreamBuffer};

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The Map Core (Stream Version)
///
/// Here `T` is the input type, and `S` is the
/// output type.  A provided (combinatorial) function
/// performs the mapping function.
pub struct Map<T: Digital, S: Digital> {
    input_buffer: StreamBuffer<T>,
    func: Func<T, S>,
}

impl<T, S> Map<T, S>
where
    T: Digital,
    S: Digital,
{
    /// Construct a Map Stream
    ///
    /// The argument to the map stream `try_new` function
    /// is a synthesizable function (i.e., one marked with the
    /// `#[kernel]` attribute).  It must have a signature of
    /// `fn(ClockReset, T) -> S`.
    pub fn try_new<K>() -> Result<Self, RHDLError>
    where
        K: DigitalFn,
        K: DigitalFn2<A0 = ClockReset, A1 = T, O = S>,
    {
        Ok(Self {
            input_buffer: StreamBuffer::default(),
            func: Func::try_new::<K>()?,
        })
    }
}

/// The input for the [Map]
pub type In<T, S> = StreamIO<T, S>;

/// The output for the [Map]
pub type Out<T, S> = StreamIO<S, T>;

impl<T, S> SynchronousIO for Map<T, S>
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
    d.input_buffer.ready = ready_cast::<T, S>(i.ready);
    let o_data = if let Some(data) = q.input_buffer.data {
        d.func = data;
        Some(q.func)
    } else {
        d.func = T::dont_care();
        None
    };
    let o = Out::<T, S> {
        data: o_data,
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
    fn map_item(_cr: ClockReset, t: b4) -> b2 {
        lsbs::<2, 4>(t)
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Map::try_new::<map_item>()?;
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// `map` wires its input buffer's `ready` straight to the
    /// downstream — the same shape that deadlocked `filter` and
    /// `filter_map`. It is safe only because it never drops: its output
    /// is `None` exactly when the buffer is empty, and then there is
    /// nothing to consume.
    ///
    /// That is safety **by accident of the operation**, not by design,
    /// so it is worth pinning down. If `map` ever gains a path that
    /// withholds output while holding an item, this test fails.
    ///
    /// Driven with `run_fn` rather than `single_stage`, because
    /// `SinkFromFn` hands its closure an acceptance report rather than
    /// the offered value and so cannot express a data-gated sink at all
    /// — see `stream::testing::sinks` for why that matters.
    #[test]
    fn map_survives_a_data_gated_sink() -> Result<(), RHDLError> {
        use crate::stream::{StreamIO, ready};
        use rhdl::core::sim::ResetOrData;

        const COUNT: u128 = 16;
        let uut = Map::<b4, b2>::try_new::<map_item>()?;
        let mut to_send: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                // Ready only when something is actually presented.
                let sink_ready = output.data.is_some();
                if let Some(d) = output.data {
                    got.push(d.raw());
                }
                let mut input = StreamIO::<b4, b2> {
                    data: None,
                    ready: ready::<b2>(sink_ready),
                };
                if to_send < COUNT && output.ready.raw {
                    input.data = Some(b4(to_send % 16));
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 200_000)
        .for_each(drop);

        assert_eq!(to_send, COUNT, "the source must not stall forever");
        let want: Vec<u128> = (0..COUNT).map(|k| k & 0x3).collect();
        assert_eq!(
            got, want,
            "map must deliver every item to a data-gated sink"
        );
        Ok(())
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// Gaps the offered data on one cadence and withholds `ready` on
    /// another, coprime to it, so the two drift and the trace covers all
    /// four combinations of (offer, accept) rather than only the aligned
    /// ones. Equal cadences would put them in lockstep and leave the
    /// held-item path untested.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4, b2>)>> {
        (0..24u128)
            .map(|k| In::<b4, b2> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b4(k % 16))
                },
                ready: crate::stream::ready::<b2>(!k.is_multiple_of(3)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Top module only; `StreamBuffer` and `Func` carry their own
    /// snapshots, and inlining them here would make this fail for
    /// changes with nothing to do with `map`.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Map::<b4, b2>::try_new::<map_item>()?;
        let desc = uut.descriptor("stream_map".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_map")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_map(input wire [1:0] clock_reset, input wire [5:0] i, output wire [3:0] o);
               wire [13:0] od;
               wire [9:0] d;
               wire [7:0] q;
               assign o = od[3:0];
               stream_map_input_buffer c0(.clock_reset(clock_reset), .i(d[5:0]), .o(q[5:0]));
               stream_map_func c1(.clock_reset(clock_reset), .i(d[9:6]), .o(q[7:6]));
               assign d = od[13:4];
               assign od = kernel_kernel(clock_reset, i, q);
               function [13:0] kernel_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [7:0] arg_2);
                     reg [4:0] r0;
                     reg [5:0] r1;
                     // d
                     reg [9:0] r2;
                     reg [0:0] r3;
                     reg [0:0] r4;
                     reg [0:0] r5;
                     // d
                     reg [9:0] r6;
                     reg [5:0] r7;
                     reg [7:0] r8;
                     reg [4:0] r9;
                     reg [0:0] r10;
                     reg [3:0] r11;
                     // d
                     reg [9:0] r12;
                     reg [1:0] r13;
                     reg [2:0] r14;
                     reg [1:0] r15;
                     // d
                     reg [9:0] r16;
                     // d
                     reg [9:0] r17;
                     reg [2:0] r18;
                     reg [5:0] r19;
                     reg [0:0] r20;
                     reg [3:0] r21;
                     reg [3:0] r22;
                     reg [13:0] r23;
                     reg [1:0] r24;
                     localparam l0 = 10'bXXXXXXXXXX;
                     localparam l1 = 1'b0;
                     localparam l2 = 1'b1;
                     localparam l3 = 4'bXXXX;
                     localparam l4 = 1'b1;
                     localparam l5 = 3'b000;
                     localparam l6 = 4'b0000;
                     begin
                        r24 = arg_0;
                        r1 = arg_1;
                        r8 = arg_2;
                        r0 = r1[4:0];
                        r2 = l0;
                        r2[4:0] = r0;
                        r3 = r1[5:5];
                        r4 = l1;
                        r5 = r4;
                        r5[0:0] = r3;
                        r6 = r2;
                        r6[5:5] = r5;
                        r7 = r8[5:0];
                        r9 = r7[4:0];
                        r10 = r9[4:4];
                        r11 = r9[3:0];
                        r12 = r6;
                        r12[9:6] = r11;
                        r13 = r8[7:6];
                        r15 = r13[1:0];
                        r14 = {l2, r15};
                        r16 = r6;
                        r16[9:6] = l3;
                        case (r10)
                           1'b1 : r17 = r12;
                           default : r17 = r16;
                        endcase
                        case (r10)
                           1'b1 : r18 = r14;
                           default : r18 = l5;
                        endcase
                        r19 = r8[5:0];
                        r20 = r19[5:5];
                        r21 = l6;
                        r21[2:0] = r18;
                        r22 = r21;
                        r22[3:3] = r20;
                        r23 = {r17, r22};
                        kernel_kernel = r23;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    ///
    /// Both paths: the RTL form skips the Stage-3 NTL passes, so an
    /// RTL-only round-trip cannot catch a bug in those passes.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = Map::<b4, b2>::try_new::<map_item>()?;
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
        let uut = Map::<b4, b2>::try_new::<map_item>()?;
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_map");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "5ef9668434775def725880681d616e185cefa61ad1580d772ea4b399ad7857b8"
        ];
        let digest = vcd.dump_to_file(root.join("stream_map.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let mut b_rng = a_rng.clone();
        let a_rng = stalling(a_rng, 0.23);
        let consume = move |v: SinkView<b2>| {
            if let Some(data) = v.accepted {
                let orig = b_rng.next().unwrap();
                let orig_lsb = lsbs::<2, 4>(orig);
                assert_eq!(data, orig_lsb);
            }
            rand::random::<f64>() > 0.2
        };
        let map = Map::try_new::<map_item>()?;
        let uut = single_stage(map, a_rng, consume);
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
