//! Zip Stream Core
//!
//!# Purpose
//!
//! A [Zip] Core takes 2 streams as inputs and yields a
//! single pipeline of outputs consisting of tuples formed
//! from the two input pipelines.  It is roughly equivalent to
//! the `.zip()` method on iterators.  The [Zip] propogates
//! backpressure up the two source pipes.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Zip] core
//!
#![doc = badascii_formal!("
        +--+Zip+---------+        
  ?S    |                |        
+------>|a.data          |        
 R<S>   |                | ?(S,T) 
 <------+a.ready     data+------> 
  ?T    |                | R<(S,T)>       
+------>|b.data     ready|<------+
 R<T>   |                |        
 <------+b.ready         |        
        +----------------+        
")]
//!
//!# Internals
//!
//! The [Zip] uses input FIFOs to buffer incoming data elements
//! on each of the two upstream pipes, and then advances them to the
//! output FIFO when both are ready.  Otherwise, the control logic
//! is straightfoward, and purely combinatorial.
//!
#![doc = badascii!(r"
      ++Stm2FIFO+--+    ++unpck+-+     +conct++                                        
  ?S  |            | ?S |        |  S  |      |      +-+pack+-+      ++FIFO2Stm+       
+---->| data  data +--->|in   out+---->|.0    |      |        |?(S,T)|         | ?(S,T)
 R<S> |            |    |        |     |   out+----->|data out+----->|in    out+------>
 <----+ ready next |<+  |     tag+-+ +>|.1    |      |        |      |         | R<(S,T)>      
      |            | |  |        | | | |      |   +->|tag     |   +--+full  rdy|<-----+
      +------------+ |  +--------+ | | +------+   |  |        |   |  |         |       
                     |             +-+---------+  |  +--------+   |  +---------+       
      ++Stm2FIFO+--+ +  ++unpck+-+   |         v  |               |                    
  ?T  |            | ?T |        | T |    +-------+-------+       |                    
+---->| data  data +--->|in   out+---+    |               |       |                    
 R<T> |            | +  |        |        |    Control    |<------+                    
 <----+ ready next |<+  |     tag+------->|               |                            
      |            | |  |        |        |               |                            
      +------------+ |  +--------+        +-+-------------+                            
                     |                      |                                          
                     +----------------------+                                          
")]
//!
//!# Example
//!
//! An example of using a [Zip] is here.
//!
//!```
#![doc = include_str!("../../examples/zip.rs")]
//!```
//!
//! With the resulting trace.
//!
#![doc = include_str!("../../doc/zip.md")]

use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::stream::{fifo_to_stream::FIFOToStream, stream_to_fifo::StreamToFIFO};

use super::Ready;

#[derive(Debug, Clone, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// The [Zip] Core
///
/// This core takes two streams.  One of type
/// `S`, and one of type `T`, and generates a stream
/// of `(S,T)` elements.
pub struct Zip<S: Digital, T: Digital> {
    a_buffer: StreamToFIFO<S>,
    b_buffer: StreamToFIFO<T>,
    out_buffer: FIFOToStream<(S, T)>,
}

#[derive(PartialEq, Clone, Copy, Digital)]
/// Input struct for the [Zip]
pub struct In<S: Digital, T: Digital> {
    /// Input data for the `a` stream
    pub a_data: Option<S>,
    /// Input data for the `b` stream
    pub b_data: Option<T>,
    /// Ready signal for the downstream
    pub ready: Ready<(S, T)>,
}

#[derive(PartialEq, Clone, Copy, Digital)]
/// Output struct for the [Zip]
pub struct Out<S: Digital, T: Digital> {
    /// Ready signal for the `a`` stream
    pub a_ready: Ready<S>,
    /// Ready signal for the `b` stream
    pub b_ready: Ready<T>,
    /// Output data containing the tuples
    pub data: Option<(S, T)>,
}

impl<S: Digital, T: Digital> SynchronousIO for Zip<S, T> {
    type I = In<S, T>;
    type O = Out<S, T>;
    type Kernel = kernel<S, T>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<S: Digital, T: Digital>(
    _cr: ClockReset,
    i: In<S, T>,
    q: Q<S, T>,
) -> (Out<S, T>, D<S, T>) {
    let mut d = D::<S, T>::dont_care();
    d.a_buffer.data = i.a_data;
    d.b_buffer.data = i.b_data;
    let mut out_data = None;
    let mut next = false;
    if !q.out_buffer.full {
        if let Some::<S>(data_a) = q.a_buffer.data {
            if let Some::<T>(data_b) = q.b_buffer.data {
                out_data = Some((data_a, data_b));
                next = true;
            }
        }
    }
    d.a_buffer.next = next;
    d.b_buffer.next = next;
    d.out_buffer.data = out_data;
    d.out_buffer.ready = i.ready;
    let o = Out::<S, T> {
        a_ready: q.a_buffer.ready,
        b_ready: q.b_buffer.ready,
        data: q.out_buffer.data,
    };
    (o, d)
}

#[cfg(test)]
mod tests {

    /// Both inputs data-gated **and** skewed against each other.
    ///
    /// `Zip` holds one side while waiting for the other, so it presents
    /// `None` downstream while still needing to accept — the
    /// absorb-without-emitting shape that deadlocked `stream::filter`.
    /// A sink withholding `ready` when it sees nothing is legal, and
    /// pairs must still come out index-aligned however skewed the
    /// arrivals are.
    #[test]
    fn data_gated_sink_does_not_stall_or_shear_the_zip() -> Result<(), RHDLError> {
        use crate::stream::ready;
        use rhdl::core::sim::ResetOrData;

        const COUNT: u128 = 12;
        let uut = Zip::<b4, b4>::default();
        let (mut a_sent, mut b_sent) = (0u128, 0u128);
        let mut got: Vec<(u128, u128)> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = output.data.is_some();
                if let Some((x, y)) = output.data {
                    got.push((x.raw(), y.raw()));
                }
                let mut input = super::In::<b4, b4> {
                    a_data: None,
                    b_data: None,
                    ready: ready::<(b4, b4)>(sink_ready),
                };
                if a_sent < COUNT && output.a_ready.raw {
                    input.a_data = Some(b4(a_sent % 16));
                    a_sent += 1;
                }
                // `b` offers on a different cadence, so the sides skew.
                if b_sent < COUNT && output.b_ready.raw && !phase.is_multiple_of(3) {
                    input.b_data = Some(b4((15 - b_sent) % 16));
                    b_sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);

        let want: Vec<(u128, u128)> = (0..COUNT).map(|k| (k % 16, (15 - k) % 16)).collect();
        assert_eq!(
            got, want,
            "pairs must stay index-aligned and none may be lost"
        );
        Ok(())
    }
    use std::iter::repeat_n;

    use super::*;
    use crate::{
        rng::xorshift::XorShift128,
        stream::testing::{
            sink_from_fn::SinkFromFn, source_from_fn::SourceFromFn, utils::stalling,
        },
    };

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct TestFixture {
        a_source: SourceFromFn<b4>,
        b_source: SourceFromFn<b6>,
        zip: Zip<b4, b6>,
        sink: SinkFromFn<(b4, b6)>,
    }

    impl SynchronousIO for TestFixture {
        type I = ();
        type O = ();
        type Kernel = kernel;
    }

    #[kernel]
    pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> ((), D) {
        let mut d = D::dont_care();
        d.zip.a_data = q.a_source;
        d.zip.b_data = q.b_source;
        d.sink = q.zip.data;
        d.zip.ready = q.sink;
        d.a_source = q.zip.a_ready;
        d.b_source = q.zip.b_ready;
        ((), d)
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// The two inputs arrive on **different, coprime cadences** (4 and
    /// 5) so they skew against each other. A zip holds one side while
    /// waiting for the other; feeding both at the same rate would keep
    /// them in step and never exercise the holding path, which is the
    /// only interesting thing the widget does.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4, b2>)>> {
        (0..30u128)
            .map(|k| In::<b4, b2> {
                a_data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b4(k % 16))
                },
                b_data: if k.is_multiple_of(5) {
                    None
                } else {
                    Some(b2(k % 4))
                },
                ready: crate::stream::ready::<(b4, b2)>(!k.is_multiple_of(3)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Zip::<b4, b2>::default();
        let desc = uut.descriptor("stream_zip".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_zip")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_zip(input wire [1:0] clock_reset, input wire [8:0] i, output wire [8:0] o);
               wire [26:0] od;
               wire [17:0] d;
               wire [20:0] q;
               assign o = od[8:0];
               stream_zip_a_buffer c0(.clock_reset(clock_reset), .i(d[5:0]), .o(q[6:0]));
               stream_zip_b_buffer c1(.clock_reset(clock_reset), .i(d[9:6]), .o(q[11:7]));
               stream_zip_out_buffer c2(.clock_reset(clock_reset), .i(d[17:10]), .o(q[20:12]));
               assign d = od[26:9];
               assign od = kernel_kernel(clock_reset, i, q);
               function [26:0] kernel_kernel(input reg [1:0] arg_0, input reg [8:0] arg_1, input reg [20:0] arg_2);
                     reg [4:0] r0;
                     reg [8:0] r1;
                     // d
                     reg [17:0] r2;
                     reg [2:0] r3;
                     // d
                     reg [17:0] r4;
                     reg [8:0] r5;
                     reg [20:0] r6;
                     reg [0:0] r7;
                     reg [0:0] r8;
                     reg [6:0] r9;
                     reg [4:0] r10;
                     reg [0:0] r11;
                     reg [3:0] r12;
                     reg [4:0] r13;
                     reg [2:0] r14;
                     reg [0:0] r15;
                     reg [1:0] r16;
                     reg [5:0] r17;
                     reg [6:0] r18;
                     reg [5:0] r19;
                     // next
                     reg [0:0] r20;
                     // out_data
                     reg [6:0] r21;
                     // next
                     reg [0:0] r22;
                     // out_data
                     reg [6:0] r23;
                     // next
                     reg [0:0] r24;
                     // out_data
                     reg [6:0] r25;
                     // d
                     reg [17:0] r26;
                     // d
                     reg [17:0] r27;
                     // d
                     reg [17:0] r28;
                     reg [0:0] r29;
                     // d
                     reg [17:0] r30;
                     reg [6:0] r31;
                     reg [0:0] r32;
                     reg [4:0] r33;
                     reg [0:0] r34;
                     reg [8:0] r35;
                     reg [6:0] r36;
                     reg [8:0] r37;
                     reg [8:0] r38;
                     reg [8:0] r39;
                     reg [26:0] r40;
                     reg [1:0] r41;
                     localparam l0 = 18'bXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b0;
                     localparam l5 = 7'b0000000;
                     localparam l6 = 1'b1;
                     localparam l7 = 9'b000000000;
                     begin
                        r41 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[4:0];
                        r2 = l0;
                        r2[4:0] = r0;
                        r3 = r1[7:5];
                        r4 = r2;
                        r4[8:6] = r3;
                        r5 = r6[20:12];
                        r7 = r5[7:7];
                        r8 = ~r7;
                        r9 = r6[6:0];
                        r10 = r9[4:0];
                        r11 = r10[4:4];
                        r12 = r10[3:0];
                        r13 = r6[11:7];
                        r14 = r13[2:0];
                        r15 = r14[2:2];
                        r16 = r14[1:0];
                        r17 = {r16, r12};
                        r19 = r17[5:0];
                        r18 = {l1, r19};
                        case (r15)
                           1'b1 : r20 = l3;
                           default : r20 = l4;
                        endcase
                        case (r15)
                           1'b1 : r21 = r18;
                           default : r21 = l5;
                        endcase
                        case (r11)
                           1'b1 : r22 = r20;
                           default : r22 = l4;
                        endcase
                        case (r11)
                           1'b1 : r23 = r21;
                           default : r23 = l5;
                        endcase
                        r24 = r8 ? r22 : l4;
                        r25 = r8 ? r23 : l5;
                        r26 = r4;
                        r26[5:5] = r24;
                        r27 = r26;
                        r27[9:9] = r24;
                        r28 = r27;
                        r28[16:10] = r25;
                        r29 = r1[8:8];
                        r30 = r28;
                        r30[17:17] = r29;
                        r31 = r6[6:0];
                        r32 = r31[5:5];
                        r33 = r6[11:7];
                        r34 = r33[3:3];
                        r35 = r6[20:12];
                        r36 = r35[6:0];
                        r37 = l7;
                        r37[0:0] = r32;
                        r38 = r37;
                        r38[1:1] = r34;
                        r39 = r38;
                        r39[8:2] = r36;
                        r40 = {r30, r39};
                        kernel_kernel = r40;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = Zip::<b4, b2>::default();
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
        let uut = Zip::<b4, b2>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_zip");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "3f58fc63e7e2c1581ea857995e3ea374e2ee229fb64658453753cc84d86c42e0"
        ];
        let digest = vcd.dump_to_file(root.join("stream_zip.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
        let b_rng = XorShift128::default().map(|x| b6(((x >> 8) & 0x3F) as u128));
        let c_rng = a_rng.clone().zip(b_rng.clone());
        let a_rng = stalling(a_rng, 0.23);
        let b_rng = stalling(b_rng, 0.15);
        let uut = TestFixture {
            a_source: SourceFromFn::new(a_rng),
            b_source: SourceFromFn::new(b_rng),
            zip: Zip::default(),
            sink: SinkFromFn::new_from_iter(c_rng, 0.2),
        };
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
