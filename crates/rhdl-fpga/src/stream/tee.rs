//! Tee Stream Core
//!
//!# Purpose
//!
//! A [Tee] Core takes a single stream as input
//! and yields two streams of outputs.  It is roughly
//! equivalent to `.unzip()` method on iterators.  The
//! [Tee] will merge backpressure from the two
//! destination streams.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Tee] core
//!
#![doc = badascii_formal!("
         +--+Tee+--------+       
  ?(S,T) |               | ?S    
+------->|data    a.data +------>
 R<(S,T)>|               | R<S>      
 <-------+ready   a.ready|<-----+
         |               | ?T    
         |        b.data +------>
         |               | R<T>      
         |        b.ready|<-----+
         +---------------+          
")]
//!
//!# Internals
//!
//! The [Tee] contains a couple of buffers and
//! a combinatorial block to split the `Option<(S,T)>`
//! into `Option<S>` and `Option<T>`.
//!
#![doc = badascii!("
                                                       ++pack++      ++FIFO2RV+-+     
                          +unpck+++     +split+ S      |      |      |          | ?S  
        ++Stm2FIFO++      |       |(S,T)|   .0+------->|in out+----->|data  data+---->
  ?(S,T)|          |?(S,T)|   data+---->|in   | T      |      |      |          | R<S>    
 +----->|data  data+----->|in     |     |   .1+--+ +-->|tag   |  +---+full ready|<---+
R<(S,T)>|          |      |    tag+-+   |     |  | |   +------+  |   |          |     
<-------+ready next|<-+   |       | |   +-----+  | |             |   +----------+     
        |          |  |   +-------+ |            | |             |                    
        +----------+  |             v            | |             |                    
                      |      +----------+        | |   ++pack++  |   ++FIFO2RV+-+     
                      |   run| Control  |        | +   |      |  |   |          | ?T  
                      +------+          +----+   +---->|in out+-+v+->|data  data+---->
                             |      full|    |     +   |      |      |          | R<T>    
                             +----------+    +-----+-->|tag   |  OR+-+full ready|<---+
                                     ^                 +------+  +   |          |     
                                     |                           |   +----------+     
                                     +---------------------------+
")]
//!
//!# Example
//!
//! Here is an example of running the tee filter.
//!
//!```
#![doc = include_str!("../../examples/tee.rs")]
//!```
//!
//! With the resulting trace.
//!
#![doc = include_str!("../../doc/tee.md")]

use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::stream::{fifo_to_stream::FIFOToStream, stream_to_fifo::StreamToFIFO};

use super::Ready;

#[derive(Debug, Clone, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// The [Tee] Core
///
/// This core takes a single stream of type `(S,T)`, and connects to
/// two outgoing streams of type `S` and `T`.
pub struct Tee<S: Digital, T: Digital> {
    in_buffer: StreamToFIFO<(S, T)>,
    s_buffer: FIFOToStream<S>,
    t_buffer: FIFOToStream<T>,
}

/// Input struct for the [Tee]
#[derive(PartialEq, Clone, Copy, Digital)]
pub struct In<S: Digital, T: Digital> {
    /// The input data for the [Tee]
    pub data: Option<(S, T)>,
    /// The downstream ready signal for the S stream
    pub s_ready: Ready<S>,
    /// The downstream ready signal for the T stream
    pub t_ready: Ready<T>,
}

/// Output struct for the [Tee]
#[derive(PartialEq, Clone, Copy, Digital)]
pub struct Out<S: Digital, T: Digital> {
    /// The output data for the S stream
    pub s_data: Option<S>,
    /// The output data for the T stream
    pub t_data: Option<T>,
    /// The upstream ready signal
    pub ready: Ready<(S, T)>,
}

impl<S: Digital, T: Digital> SynchronousIO for Tee<S, T> {
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
    let mut s_val = None;
    let mut t_val = None;
    let full = q.s_buffer.full || q.t_buffer.full;
    let mut next = false;
    if !full {
        if let Some(data) = q.in_buffer.data {
            s_val = Some(data.0);
            t_val = Some(data.1);
            next = true;
        }
    }
    d.s_buffer.data = s_val;
    d.t_buffer.data = t_val;
    d.in_buffer.next = next;
    d.in_buffer.data = i.data;
    d.s_buffer.ready = i.s_ready;
    d.t_buffer.ready = i.t_ready;
    let o = Out::<S, T> {
        s_data: q.s_buffer.data,
        t_data: q.t_buffer.data,
        ready: q.in_buffer.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {

    /// Both outputs data-gated, draining at different rates.
    ///
    /// `Tee` cannot emit either half until both sides can take one, so
    /// it presents `None` while holding an item — the
    /// absorb-without-emitting shape that deadlocked `stream::filter`.
    /// Each branch withholds `ready` when it sees nothing, and both
    /// halves must still arrive complete and aligned.
    #[test]
    fn data_gated_sinks_do_not_stall_or_desync_the_tee() -> Result<(), RHDLError> {
        use crate::stream::ready;
        use rhdl::core::sim::ResetOrData;

        const COUNT: u128 = 12;
        let uut = Tee::<b4, b4>::default();
        let mut sent: u128 = 0;
        let (mut got_s, mut got_t) = (Vec::<u128>::new(), Vec::<u128>::new());
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                // Both gated on seeing data; `t` additionally throttled
                // so the branches drain at different rates.
                let s_ready = output.s_data.is_some();
                let t_ready = output.t_data.is_some() && !phase.is_multiple_of(3);
                if s_ready {
                    if let Some(d) = output.s_data {
                        got_s.push(d.raw());
                    }
                }
                if t_ready {
                    if let Some(d) = output.t_data {
                        got_t.push(d.raw());
                    }
                }
                let mut input = super::In::<b4, b4> {
                    data: None,
                    s_ready: ready::<b4>(s_ready),
                    t_ready: ready::<b4>(t_ready),
                };
                if sent < COUNT && output.ready.raw {
                    input.data = Some((b4(sent % 16), b4((15 - sent) % 16)));
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);

        let want_s: Vec<u128> = (0..COUNT).map(|k| k % 16).collect();
        let want_t: Vec<u128> = (0..COUNT).map(|k| (15 - k) % 16).collect();
        assert_eq!(
            got_s, want_s,
            "the s branch must arrive complete and in order"
        );
        assert_eq!(
            got_t, want_t,
            "the t branch must arrive complete and in order"
        );
        Ok(())
    }
    use std::iter::repeat_n;

    use rhdl::core::SynchronousIO;

    use super::Tee;
    use super::*;
    use crate::rng::xorshift::XorShift128;
    use crate::stream::testing::sink_from_fn::{SinkFromFn, SinkView};
    use crate::stream::testing::source_from_fn::SourceFromFn;
    use crate::stream::testing::utils::stalling;

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct TestFixture {
        source: SourceFromFn<(b4, b6)>,
        tee: Tee<b4, b6>,
        s_sink: SinkFromFn<b4>,
        t_sink: SinkFromFn<b6>,
    }

    impl SynchronousIO for TestFixture {
        type I = ();
        type O = ();
        type Kernel = kernel;
    }

    #[kernel]
    pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> ((), D) {
        let mut d = D::dont_care();
        d.tee.data = q.source;
        d.source = q.tee.ready;
        d.s_sink = q.tee.s_data;
        d.t_sink = q.tee.t_data;
        d.tee.s_ready = q.s_sink;
        d.tee.t_ready = q.t_sink;
        ((), d)
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// The two branches drain at **different, coprime rates** (3 and 5).
    /// A tee cannot emit either half until both sides can take one, so
    /// equal rates would let both branches stall and resume together and
    /// never exercise the case the widget exists to handle: one branch
    /// blocked while the other is free.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4, b2>)>> {
        (0..30u128)
            .map(|k| In::<b4, b2> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some((b4(k % 16), b2(k % 4)))
                },
                s_ready: crate::stream::ready::<b4>(!k.is_multiple_of(3)),
                t_ready: crate::stream::ready::<b2>(!k.is_multiple_of(5)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Tee::<b4, b2>::default();
        let desc = uut.descriptor("stream_tee".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_tee")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_tee(input wire [1:0] clock_reset, input wire [8:0] i, output wire [8:0] o);
               wire [26:0] od;
               wire [17:0] d;
               wire [20:0] q;
               assign o = od[8:0];
               stream_tee_in_buffer c0(.clock_reset(clock_reset), .i(d[7:0]), .o(q[8:0]));
               stream_tee_s_buffer c1(.clock_reset(clock_reset), .i(d[13:8]), .o(q[15:9]));
               stream_tee_t_buffer c2(.clock_reset(clock_reset), .i(d[17:14]), .o(q[20:16]));
               assign d = od[26:9];
               assign od = kernel_kernel(clock_reset, i, q);
               function [26:0] kernel_kernel(input reg [1:0] arg_0, input reg [8:0] arg_1, input reg [20:0] arg_2);
                     reg [6:0] r0;
                     reg [20:0] r1;
                     reg [0:0] r2;
                     reg [4:0] r3;
                     reg [0:0] r4;
                     reg [0:0] r5;
                     reg [0:0] r6;
                     reg [8:0] r7;
                     reg [6:0] r8;
                     reg [0:0] r9;
                     reg [5:0] r10;
                     reg [3:0] r11;
                     reg [4:0] r12;
                     reg [3:0] r13;
                     reg [1:0] r14;
                     reg [2:0] r15;
                     reg [1:0] r16;
                     // next
                     reg [0:0] r17;
                     // s_val
                     reg [4:0] r18;
                     // t_val
                     reg [2:0] r19;
                     // next
                     reg [0:0] r20;
                     // s_val
                     reg [4:0] r21;
                     // t_val
                     reg [2:0] r22;
                     // d
                     reg [17:0] r23;
                     // d
                     reg [17:0] r24;
                     // d
                     reg [17:0] r25;
                     reg [6:0] r26;
                     reg [8:0] r27;
                     // d
                     reg [17:0] r28;
                     reg [0:0] r29;
                     // d
                     reg [17:0] r30;
                     reg [0:0] r31;
                     // d
                     reg [17:0] r32;
                     reg [6:0] r33;
                     reg [4:0] r34;
                     reg [4:0] r35;
                     reg [2:0] r36;
                     reg [8:0] r37;
                     reg [0:0] r38;
                     reg [8:0] r39;
                     reg [8:0] r40;
                     reg [8:0] r41;
                     reg [26:0] r42;
                     reg [1:0] r43;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b0;
                     localparam l5 = 5'b00000;
                     localparam l6 = 3'b000;
                     localparam l7 = 18'bXXXXXXXXXXXXXXXXXX;
                     localparam l8 = 9'b000000000;
                     begin
                        r43 = arg_0;
                        r27 = arg_1;
                        r1 = arg_2;
                        r0 = r1[15:9];
                        r2 = r0[5:5];
                        r3 = r1[20:16];
                        r4 = r3[3:3];
                        r5 = r2 | r4;
                        r6 = ~r5;
                        r7 = r1[8:0];
                        r8 = r7[6:0];
                        r9 = r8[6:6];
                        r10 = r8[5:0];
                        r11 = r10[3:0];
                        r13 = r11[3:0];
                        r12 = {l0, r13};
                        r14 = r10[5:4];
                        r16 = r14[1:0];
                        r15 = {l1, r16};
                        case (r9)
                           1'b1 : r17 = l3;
                           default : r17 = l4;
                        endcase
                        case (r9)
                           1'b1 : r18 = r12;
                           default : r18 = l5;
                        endcase
                        case (r9)
                           1'b1 : r19 = r15;
                           default : r19 = l6;
                        endcase
                        r20 = r6 ? r17 : l4;
                        r21 = r6 ? r18 : l5;
                        r22 = r6 ? r19 : l6;
                        r23 = l7;
                        r23[12:8] = r21;
                        r24 = r23;
                        r24[16:14] = r22;
                        r25 = r24;
                        r25[7:7] = r20;
                        r26 = r27[6:0];
                        r28 = r25;
                        r28[6:0] = r26;
                        r29 = r27[7:7];
                        r30 = r28;
                        r30[13:13] = r29;
                        r31 = r27[8:8];
                        r32 = r30;
                        r32[17:17] = r31;
                        r33 = r1[15:9];
                        r34 = r33[4:0];
                        r35 = r1[20:16];
                        r36 = r35[2:0];
                        r37 = r1[8:0];
                        r38 = r37[7:7];
                        r39 = l8;
                        r39[4:0] = r34;
                        r40 = r39;
                        r40[7:5] = r36;
                        r41 = r40;
                        r41[8:8] = r38;
                        r42 = {r32, r41};
                        kernel_kernel = r42;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = Tee::<b4, b2>::default();
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
        let uut = Tee::<b4, b2>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_tee");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "5bdc2ddd668e042b559181db0faa6dca218b47c658b70103543d6d65968a2c61"
        ];
        let digest = vcd.dump_to_file(root.join("stream_tee.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default().map(|x| {
            let s = b4((x & 0xF) as u128);
            let t = b6(((x >> 8) & 0x3F) as u128);
            (s, t)
        });
        let mut c_rng = a_rng.clone();
        let mut d_rng = a_rng.clone();
        let a_rng = stalling(a_rng, 0.23);
        let consume_s = move |v: SinkView<_>| {
            if let Some(data) = v.accepted {
                let validation = c_rng.next().unwrap();
                assert_eq!(data, validation.0);
            }
            rand::random::<f64>() > 0.2
        };
        let consume_t = move |v: SinkView<_>| {
            if let Some(data) = v.accepted {
                let validation = d_rng.next().unwrap();
                assert_eq!(data, validation.1);
            }
            rand::random::<f64>() > 0.2
        };
        let uut = TestFixture {
            source: SourceFromFn::new(a_rng),
            tee: Tee::default(),
            s_sink: SinkFromFn::new(consume_s),
            t_sink: SinkFromFn::new(consume_t),
        };
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
