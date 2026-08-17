//! Wrap a Pipe into a Stream
//!
//!# Purpose
//!
//! This core is used to take a pipeline with no backpressure, and interface it into a stream.
//! The backpressure is handled by an internal FIFO where output elements of the pipe are
//! allocated space (like a credit-based system in networking).  
//!
//!# Details
//!
//! The original Latency Insensitive Design work focused on stallable pipelines.  That
//! is to say, that if the `ready` signal was taken away (or equivalently, if the
//! downstream process asserted `stop`), then the entire pipeline would stall
//! until the `ready` signal was reasserted.  In the original papers, this was done via
//! a gated clock or a clock-enable signal that was used to either advance a given
//! stage in the pipeline or hold it in it's current state.  Roughly, something like
//! this:
#![doc = badascii!(r"
      +--+Pipeline+--+          
  ?S  |              | ?T        
+---->|in        out +---->     
      |              |          
      |     clk_en   |          
      +--------------+          
              ^                 
              |          ready  
              +----------------+
")]
//!
//! However, the idea of a `clk_en` line doesn't always fit with a pipeline.  For example,
//! a DRAM controller can be seen as a pipeline (where `S` is the address to read from and
//! `T` are the data elements read back, for example).  The DRAM controller is generally
//! not stallable.  And it is fair to assume that the controller requires you to read out the data
//! elements from the output once you have committed a certain transaction.  
//!
//! Furthermore, suppose that each item `S` injected into the pipeline produces `N` items of
//! type `T` on the output of the pipeline. Then when can a new item be injected?  If the pipeline
//! is opaque, then we can only keep track of how many pending items need to be written to the
//! output.
//!
//! The obvious answer is to include an output FIFO at the end of the pipeline to hold the
//! items as they are produced by the pipeline.  These can then be served to the downstream
//! process as it manages the `ready` signal.
//!
#![doc = badascii!(r"
      +--+Pipeline+--+     +--+FIFO+---+      
  ?S  |              | ?T  |           | ?T   
+---->|in        out +---->| in    out +----> 
      |              |     |           | ready
      |              |     |       next|<----+
      +--------------+     +-----------+      
")]
//!
//! Ignoring, temporarily the problem of underflow of the output FIFO, the bigger problem is the lack
//! of backpressure handling by the pipeline.  If the output FIFO is full, how do we stall the pipeline?
//! If it has no clock enable or other means of stalling, we are still in the same situation as before.
//!
//! The proposed solution in this core is to introduce a credit-based system.  A control core that
//! tracks the number of open slots in the output FIFO, and only dispatches as many items `S` such
//! that the output is guaranteed to fit in the output FIFO.  Each clock for which the `ready` signal
//! is asserted will release an additional credit to the controller, and each `S` item that is consumed
//! will require `N` credits to be available, where `N` is the number of `T` items produced by each `S`.
//!
//! Thus, backpressure is moved upstream of the pipeline.  The pipeline itself does not need to support
//! backpressure, since the controller will stop the inflow of data when there is insufficient credit
//! in the FIFO to start processing more data elements.
//!
//! Furthermore this design is invariant to the latency introduced by the pipeline.  It can even be variable.
//! Each output slot in the FIFO is reserved for a pending computation, and
//! credit tracking makes no assumptions about how long those reservations are held for.
//!
#![doc = badascii!(r"
                           +                                                       
  ?S +-+RV2FIFO+--+ ?S     |\                                                      
+--->|data    data+------->|1+ ?S ++Pipeline++ ?T +-+FIFO+----+ ?T +-+FIFO2RV+-+ ?T
 R<S>|            |        | +--->|in    out +--->|data   data+--->|data   data+-->
<----+ready   next|<+None+>|0+    | delay N  |    |           |    |           | R<T>  
     +------------+ |      |/     +----------+ +--+full   next|  +-+full  ready|<-+
                    |      +^    +---------+   |  +-----------+  | +-----------+   
                    |       +----+ Control |<--+            ^    |                 
                    +------------+         |<--------------+|+---+                 
                                 +--------++                |                      
                                          +-----------------+                      
")]
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [PipeWrapper].
//!
#![doc = badascii_formal!("
      +-+PipeWrapper+------+       
  ?S  |                    |  ?T   
+---->| data         data  +------>
 R<S> |                    | R<T>     
<-----+ ready        ready |<-----+
   ?S |                    |  ?T   
<-----+ to_pipe  from_pipe |<-----+
      +--------------------+       
")]
//!
//! It is understood that the pipline will start when fed `Some(S)` data
//! element, and will produce exactly one [Option<T>] output element at some
//! time in the future.  The internal FIFO size is exposed, since knowledge of how big the
//! output FIFO will need to be is a design decision.
//!
//!# Example
//!
//! An example of wrapping a pipeline with a [PipeWrapper] core
//! is here.
//!
//!```
#![doc = include_str!("../../examples/pipe_wrap.rs")]
//!```
//!
//! With the resulting trace.
//!
#![doc = include_str!("../../doc/pipe_wrap.md")]

use crate::{
    core::{dff::DFF, option::is_some},
    fifo::synchronous::SyncFIFO,
    stream::{fifo_to_stream::FIFOToStream, stream_to_fifo::StreamToFIFO},
};
use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use super::Ready;

#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// [PipeWrapper] core for wrapping a pipeline into a stream
///
/// This core allows you to run a pipeline (that accepts no backpressure)
/// inside a stream.   An internal fifo with `N` address bits is used to
/// hold reserved slots for the output of the pipeline.  The input stream
/// carries elements of type `S` and the pipeline is assumed to produce elements
/// of type `T`.  This core assumes a 1-1 relationship, i.e., each `Some(S)` will
/// produce exactly one `Some(T)`.
pub struct PipeWrapper<S: Digital, T: Digital, const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    in_buffer: StreamToFIFO<S>,
    fifo: SyncFIFO<T, N>,
    out_buffer: FIFOToStream<T>,
    counter: DFF<Bits<N>>,
}

impl<S: Digital, T: Digital, const N: usize> Default for PipeWrapper<S, T, N>
where
    rhdl::bits::W<N>: BitWidth,
{
    fn default() -> Self {
        Self {
            in_buffer: StreamToFIFO::default(),
            fifo: SyncFIFO::default(),
            out_buffer: FIFOToStream::default(),
            counter: DFF::new(Bits::<N>::MAX),
        }
    }
}

#[derive(PartialEq, Clone, Copy, Digital)]
/// Inputs for the [PipeWrapper]
pub struct In<S: Digital, T: Digital> {
    /// Input data for the upstream
    pub data: Option<S>,
    /// Input ready signal for the downstream
    pub ready: Ready<T>,
    /// The values that come from the pipeline
    pub from_pipe: Option<T>,
}

#[derive(PartialEq, Clone, Copy, Digital)]
/// Outputs from the [PipeWrapper]
pub struct Out<S: Digital, T: Digital> {
    /// Output data for the downstream
    pub data: Option<T>,
    /// Ready signal for the upstream
    pub ready: Ready<S>,
    /// Data to feed the pipeline
    pub to_pipe: Option<S>,
}

impl<S: Digital, T: Digital, const N: usize> SynchronousIO for PipeWrapper<S, T, N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = In<S, T>;
    type O = Out<S, T>;
    type Kernel = kernel<S, T, N>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<S: Digital, T: Digital, const N: usize>(
    _cr: ClockReset,
    i: In<S, T>,
    q: Q<S, T, N>,
) -> (Out<S, T>, D<S, T, N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = D::<S, T, N>::dont_care();
    // Is there a slot available?
    let is_slot_available = (q.counter > 0) && !q.out_buffer.full;
    // If the data is available and a slot is available
    // then feed it a new data element
    let mut o = Out::<S, T>::dont_care();
    o.to_pipe = None;
    let mut will_accept = false;
    if is_slot_available {
        // Is more data available to feed the pipeline?
        if let Some(s_data) = q.in_buffer.data {
            will_accept = true;
            o.to_pipe = Some(s_data);
        }
    }
    o.ready = q.in_buffer.ready;
    o.data = q.out_buffer.data;
    d.in_buffer.next = will_accept;
    d.in_buffer.data = i.data;
    d.out_buffer.ready = i.ready;
    let t_tag = is_some::<T>(q.fifo.data);
    let will_unload = t_tag && !q.out_buffer.full;
    d.fifo.next = will_unload;
    d.fifo.data = i.from_pipe;
    // Route the unloaded FIFO item into the output buffer.
    //
    // This assignment was missing: `d.out_buffer.data` was never
    // driven, so the output buffer was fed a don't-care that
    // materialised as `None` and the widget emitted nothing, ever.
    d.out_buffer.data = if will_unload { q.fifo.data } else { None };
    d.counter = match (will_accept, will_unload) {
        (false, false) => q.counter,
        (true, true) => q.counter,
        (true, false) => q.counter - 1,
        (false, true) => q.counter + 1,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use delay::DelayLine;

    use super::*;
    use crate::{
        core::{dff::DFF, option::pack, slice::lsbs},
        rng::xorshift::XorShift128,
        stream::testing::{
            sink_from_fn::{SinkFromFn, SinkView},
            source_from_fn::SourceFromFn,
            utils::stalling,
        },
    };

    pub mod delay {
        use crate::core::option::unpack;

        use super::*;
        #[derive(Clone, Synchronous, SynchronousDQ, Default)]
        #[rhdl(dq_no_prefix)]
        pub struct DelayLine {
            stage_0: DFF<Option<b6>>,
            stage_1: DFF<Option<b6>>,
            stage_2: DFF<Option<b4>>,
        }

        impl SynchronousIO for DelayLine {
            type I = Option<b6>;
            type O = Option<b4>;
            type Kernel = kernel;
        }

        #[kernel]
        pub fn kernel(_cr: ClockReset, i: Option<b6>, q: Q) -> (Option<b4>, D) {
            let mut d = D::dont_care();
            d.stage_0 = i;
            d.stage_1 = q.stage_0;
            let (tag, data) = unpack::<b6>(q.stage_1, bits(0));
            let data = lsbs::<4, 6>(data);
            d.stage_2 = pack::<b4>(tag, data);
            (q.stage_2, d)
        }
    }

    ///
    /// Here is a sketch of the internals:
    ///
    #[doc = badascii!(r"
+Source+-+    +Wrapper+-----+     +Sink+--+
|        | ?T |             | ?S  |       |
|    data+--->|data     data+---->|data   |
|        |    |             |     |       |
|   ready|<---+ready   ready|<----+ready  |
+--------+    +--+------+---+     +-------+
           +-----+      +----+             
         ?T|  +------------+ |?S           
           +->|in       out+-+             
              +------------+               
")]
    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct TestFixture {
        source: SourceFromFn<b6>,
        delay: DelayLine,
        wrapper: PipeWrapper<b6, b4, 2>,
        sink: SinkFromFn<b4>,
    }

    impl SynchronousIO for TestFixture {
        type I = ();
        type O = ();
        type Kernel = kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> ((), D) {
        let mut d = D::dont_care();
        d.wrapper.data = q.source;
        d.source = q.wrapper.ready;
        d.sink = q.wrapper.data;
        d.wrapper.ready = q.sink;
        d.delay = q.wrapper.to_pipe;
        d.wrapper.from_pipe = q.delay;
        ((), d)
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// `PipeWrapper` straddles two interfaces: a Ready/Valid stream and
    /// a fixed-latency pipeline it feeds via `to_pipe` / `from_pipe`.
    /// Three signals move on three coprime cadences (4, 3, 5) so the
    /// stream side, the sink side, and the pipeline return never line
    /// up — a wrapper whose job is holding results until the downstream
    /// takes them is uninteresting if they do.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b6, b4>)>> {
        (0..30u128)
            .map(|k| In::<b6, b4> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b6(k % 64))
                },
                ready: crate::stream::ready::<b4>(!k.is_multiple_of(3)),
                from_pipe: if k.is_multiple_of(5) {
                    None
                } else {
                    Some(b4(k % 16))
                },
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// **Regression: the wrapper delivered nothing at all.**
    ///
    /// `d.out_buffer.data` was never assigned, so the output buffer was
    /// driven by a don't-care that materialised as `None`. The widget
    /// emitted zero items — for its whole life, through its own fixture
    /// as well as standalone.
    ///
    /// It survived because the only behavioural test asserted values
    /// *inside* `if let Some(data) = v.accepted`, so a widget that never
    /// produced anything ran zero assertions and passed. Tier 3 is what
    /// finally caught it, as a partial-initialisation error at
    /// `descriptor()` — the simulator never asks for HDL and so never
    /// noticed the undriven input.
    ///
    /// This test asserts a **count**, which is what the original was
    /// missing: a property of what arrives cannot detect nothing
    /// arriving.
    #[test]
    fn wrapper_actually_delivers_items() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        SEEN.store(0, Ordering::Relaxed);
        let b_rng = XorShift128::default().map(|x| b6(((x >> 8) & 0x3F) as u128));
        let consume = move |v: SinkView<_>| {
            if v.accepted.is_some() {
                SEEN.fetch_add(1, Ordering::Relaxed);
            }
            true
        };
        let uut = TestFixture {
            source: SourceFromFn::new(b_rng.map(Some)),
            delay: DelayLine::default(),
            wrapper: PipeWrapper::default(),
            sink: SinkFromFn::new(consume),
        };
        let input = repeat_n((), 2_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        let seen = SEEN.load(Ordering::Relaxed);
        assert!(
            seen > 500,
            "the wrapper must actually deliver items; got {seen}"
        );
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = PipeWrapper::<b6, b4, 2>::default();
        let desc = uut.descriptor("pipe_wrapper".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "pipe_wrapper")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module pipe_wrapper(input wire [1:0] clock_reset, input wire [12:0] i, output wire [12:0] o);
               wire [34:0] od;
               wire [21:0] d;
               wire [27:0] q;
               assign o = od[12:0];
               pipe_wrapper_in_buffer c0(.clock_reset(clock_reset), .i(d[7:0]), .o(q[8:0]));
               pipe_wrapper_fifo c1(.clock_reset(clock_reset), .i(d[13:8]), .o(q[18:9]));
               pipe_wrapper_out_buffer c2(.clock_reset(clock_reset), .i(d[19:14]), .o(q[25:19]));
               pipe_wrapper_counter c3(.clock_reset(clock_reset), .i(d[21:20]), .o(q[27:26]));
               assign d = od[34:13];
               assign od = kernel_kernel(clock_reset, i, q);
               function [34:0] kernel_kernel(input reg [1:0] arg_0, input reg [12:0] arg_1, input reg [27:0] arg_2);
                     reg [1:0] r0;
                     reg [27:0] r1;
                     reg [0:0] r2;
                     reg [6:0] r3;
                     reg [0:0] r4;
                     reg [0:0] r5;
                     reg [0:0] r6;
                     reg [8:0] r7;
                     reg [6:0] r8;
                     reg [0:0] r9;
                     reg [5:0] r10;
                     reg [6:0] r11;
                     reg [5:0] r12;
                     // o
                     reg [12:0] r13;
                     // o
                     reg [12:0] r14;
                     // will_accept
                     reg [0:0] r15;
                     // o
                     reg [12:0] r16;
                     // will_accept
                     reg [0:0] r17;
                     reg [8:0] r18;
                     reg [0:0] r19;
                     // o
                     reg [12:0] r20;
                     reg [6:0] r21;
                     reg [4:0] r22;
                     // o
                     reg [12:0] r23;
                     // d
                     reg [21:0] r24;
                     reg [6:0] r25;
                     reg [12:0] r26;
                     // d
                     reg [21:0] r27;
                     reg [0:0] r28;
                     // d
                     reg [21:0] r29;
                     reg [9:0] r30;
                     reg [4:0] r31;
                     reg [0:0] r32;
                     reg [0:0] r33;
                     reg [6:0] r34;
                     reg [0:0] r35;
                     reg [0:0] r36;
                     reg [0:0] r37;
                     // d
                     reg [21:0] r38;
                     reg [4:0] r39;
                     // d
                     reg [21:0] r40;
                     reg [9:0] r41;
                     reg [4:0] r42;
                     reg [4:0] r43;
                     // d
                     reg [21:0] r44;
                     reg [1:0] r45;
                     reg [1:0] r46;
                     reg [1:0] r47;
                     reg [1:0] r48;
                     reg [1:0] r49;
                     reg [1:0] r50;
                     reg [1:0] r51;
                     reg [1:0] r52;
                     // d
                     reg [21:0] r53;
                     reg [34:0] r54;
                     reg [1:0] r55;
                     localparam l0 = 2'b00;
                     localparam l1 = 1'b1;
                     localparam l2 = 13'b0000000XXXXXX;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b0;
                     localparam l6 = 22'bXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l7 = 1'b1;
                     localparam l8 = 1'b1;
                     localparam l9 = 1'b0;
                     localparam l10 = 1'b0;
                     localparam l11 = 5'b00000;
                     localparam l12 = 2'b01;
                     localparam l13 = 2'b01;
                     localparam l14 = 2'b00;
                     localparam l15 = 2'b11;
                     localparam l16 = 2'b01;
                     localparam l17 = 2'b10;
                     begin
                        r55 = arg_0;
                        r26 = arg_1;
                        r1 = arg_2;
                        r0 = r1[27:26];
                        r2 = r0 > l0;
                        r3 = r1[25:19];
                        r4 = r3[5:5];
                        r5 = ~r4;
                        r6 = r2 & r5;
                        r7 = r1[8:0];
                        r8 = r7[6:0];
                        r9 = r8[6:6];
                        r10 = r8[5:0];
                        r12 = r10[5:0];
                        r11 = {l1, r12};
                        r13 = l2;
                        r13[12:6] = r11;
                        case (r9)
                           1'b1 : r14 = r13;
                           default : r14 = l2;
                        endcase
                        case (r9)
                           1'b1 : r15 = l4;
                           default : r15 = l5;
                        endcase
                        r16 = r6 ? r14 : l2;
                        r17 = r6 ? r15 : l5;
                        r18 = r1[8:0];
                        r19 = r18[7:7];
                        r20 = r16;
                        r20[5:5] = r19;
                        r21 = r1[25:19];
                        r22 = r21[4:0];
                        r23 = r20;
                        r23[4:0] = r22;
                        r24 = l6;
                        r24[7:7] = r17;
                        r25 = r26[6:0];
                        r27 = r24;
                        r27[6:0] = r25;
                        r28 = r26[7:7];
                        r29 = r27;
                        r29[19:19] = r28;
                        r30 = r1[18:9];
                        r31 = r30[4:0];
                        r32 = r31[4:4];
                        case (r32)
                           1'b1 : r33 = l8;
                           1'b0 : r33 = l10;
                        endcase
                        r34 = r1[25:19];
                        r35 = r34[5:5];
                        r36 = ~r35;
                        r37 = r33 & r36;
                        r38 = r29;
                        r38[13:13] = r37;
                        r39 = r26[12:8];
                        r40 = r38;
                        r40[12:8] = r39;
                        r41 = r1[18:9];
                        r42 = r41[4:0];
                        r43 = r37 ? r42 : l11;
                        r44 = r40;
                        r44[18:14] = r43;
                        r45 = {r37, r17};
                        r46 = r1[27:26];
                        r47 = r1[27:26];
                        r48 = r1[27:26];
                        r49 = r48 - l12;
                        r50 = r1[27:26];
                        r51 = r50 + l13;
                        case (r45)
                           2'b00 : r52 = r46;
                           2'b11 : r52 = r47;
                           2'b01 : r52 = r49;
                           2'b10 : r52 = r51;
                        endcase
                        r53 = r44;
                        r53[21:20] = r52;
                        r54 = {r53, r23};
                        kernel_kernel = r54;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = PipeWrapper::<b6, b4, 2>::default();
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
        let uut = PipeWrapper::<b6, b4, 2>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("pipe_wrapper");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "7bc6729d2c6ce28000a06f1bd999159ae2815528b7519ab1ccbd7e43c7f90d4f"
        ];
        let digest = vcd.dump_to_file(root.join("pipe_wrapper.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let b_rng = XorShift128::default().map(|x| b6(((x >> 8) & 0x3F) as u128));
        let mut c_rng = b_rng.clone();
        let b_rng = stalling(b_rng, 0.13);
        let consume = move |v: SinkView<_>| {
            if let Some(data) = v.accepted {
                let validation = lsbs::<4, 6>(c_rng.next().unwrap());
                assert_eq!(data, validation);
            }
            rand::random::<f64>() > 0.2
        };
        let uut = TestFixture {
            source: SourceFromFn::new(b_rng),
            delay: DelayLine::default(),
            wrapper: PipeWrapper::default(),
            sink: SinkFromFn::new(consume),
        };
        // Run a few samples through
        let input = repeat_n((), 10_000).with_reset(1).clock_pos_edge(100);
        uut.run(input).for_each(drop);
        Ok(())
    }
}
