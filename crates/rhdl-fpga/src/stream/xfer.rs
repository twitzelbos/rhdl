//! Xfer Detector Core
//!
//!# Purpose
//!
//! A [Xfer] Core sits on a stream, and emits a pulse
//! (combinatorially) in each clock cycle that a valid
//! transfer takes place on the stream.  It is equivalent
//! to computing `data.is_some() && ready`, but can be
//! easier to use as a block than as an expression.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [Xfer] core
//!
#![doc = badascii_formal!(r"
     +--+Xfer+--------+      
 ?T  |                | ?T   
+--->+ data     data  +----> 
Ry<T>|                | Ry<T>
<----+ ready    ready |<---+ 
     |      run       |      
     +-------+--------+      
             v               
")]
//!
//!# Internals
//!
//! This core is very simple.  It passes the data and ready
//! signals through, and derives (combinatorially) the `run`
//! signal as `data.is_some() && ready`.
//!
#![doc = badascii!(r"
 ?T                             
+------+------------------->    
       |                        
       | +-------+        run   
       +>|is_some+--> & +-->    
         +-------+              
 Ready<T>             ^ Ready<T>
<---------------------+-----+   
")]
//!
//!# Example
//!
use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::core::constant::Constant;
use crate::core::option::is_some;

use super::{Ready, StreamIO};

#[derive(Debug, Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The [Xfer] core
///
/// Emits a `run` pulse on every clock cycle where a
/// transfer takes place on a stream.  This core is unbuffered,
/// and the output is combinatorially derived from the inputs.
pub struct Xfer<T: Digital> {
    /// Zero-cost type-parameter carrier.
    ///
    /// This was a `PhantomData<T>`, which made the widget **impossible
    /// to synthesise**: `SynchronousDQ` treats every field as a child
    /// circuit, and `PhantomData` has no HDL, so `descriptor()` failed
    /// with `FunctionNotSynthesizable { name: "uut_marker" }` — for the
    /// widget alone and for any design containing it. Nothing caught it
    /// because `stream::xfer` had no Tier 3 or Tier 4 coverage; the
    /// simulator never asks for HDL.
    ///
    /// `Constant<T>` is the idiom that works (see
    /// [`crate::rcstream::credit::source::CreditSource`]): it carries
    /// the type parameter, synthesises to a constant driver with no DFF
    /// state, and the kernel ignores its output.
    marker: Constant<T>,
}

impl<T: Digital> Default for Xfer<T> {
    fn default() -> Self {
        Self {
            marker: Constant::new(T::dont_care()),
        }
    }
}

/// Output of the [Xfer] core
#[derive(PartialEq, Clone, Copy, Digital)]
pub struct Out<T: Digital> {
    /// The data flowing out of the core
    pub data: Option<T>,
    /// The ready signal flowing out of the core
    pub ready: Ready<T>,
    /// A pulse that is high when a transfer takes place
    pub run: bool,
}

impl<T: Digital> SynchronousIO for Xfer<T> {
    type I = StreamIO<T, T>;
    type O = Out<T>;
    type Kernel = kernel<T>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<T: Digital>(_cr: ClockReset, i: StreamIO<T, T>, _q: Q<T>) -> (Out<T>, D<T>) {
    let d = D::<T> { marker: () };
    let run = is_some::<T>(i.data) & i.ready.raw;
    let o = Out::<T> {
        data: i.data,
        ready: i.ready,
        run,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use super::*;
    use crate::{
        core::dff::DFF,
        rng::xorshift::XorShift128,
        stream::testing::{
            sink_from_fn::SinkFromFn, source_from_fn::SourceFromFn, utils::stalling,
        },
    };

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    pub struct TestFixture {
        source: SourceFromFn<b4>,
        count: DFF<b16>,
        xfer: Xfer<b4>,
        sink: SinkFromFn<b4>,
    }

    impl SynchronousIO for TestFixture {
        type I = ();
        type O = b16;
        type Kernel = kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn kernel(_cr: ClockReset, _i: (), q: Q) -> (b16, D) {
        let mut d = D::dont_care();
        d.source = q.xfer.ready;
        d.sink = q.xfer.data;
        d.count = q.count;
        if q.xfer.run {
            d.count += 1;
        }
        d.xfer.data = q.source;
        d.xfer.ready = q.sink;
        (q.count, d)
    }

    /// Open-loop stimulus for Tiers 3-5: offers gapped on 4, `ready`
    /// withheld on 3. `Xfer`'s `run` output pulses exactly on a
    /// transfer, so the two cadences drifting apart is what makes the
    /// trace show `run` both firing and correctly staying low when only
    /// one side of the handshake is present.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, StreamIO<b4, b4>)>> {
        (0..24u128)
            .map(|k| StreamIO::<b4, b4> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b4(k % 16))
                },
                ready: crate::stream::ready::<b4>(!k.is_multiple_of(3)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Xfer::<b4>::default();
        let desc = uut.descriptor("stream_xfer".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_xfer")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_xfer(input wire [1:0] clock_reset, input wire [5:0] i, output wire [6:0] o);
               wire [6:0] od;
               wire [3:0] q;
               assign o = od[6:0];
               stream_xfer_marker c0(.clock_reset(clock_reset), .o(q[3:0]));
               assign od = kernel_kernel(clock_reset, i, q);
               function [6:0] kernel_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [3:0] arg_2);
                     reg [4:0] r0;
                     reg [5:0] r1;
                     reg [0:0] r2;
                     reg [0:0] r3;
                     reg [0:0] r4;
                     reg [0:0] r5;
                     reg [4:0] r6;
                     reg [0:0] r7;
                     reg [6:0] r8;
                     reg [6:0] r9;
                     reg [6:0] r10;
                     reg [1:0] r11;
                     reg [3:0] r12;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b0;
                     localparam l4 = 7'b0000000;
                     begin
                        r11 = arg_0;
                        r1 = arg_1;
                        r12 = arg_2;
                        r0 = r1[4:0];
                        r2 = r0[4:4];
                        case (r2)
                           1'b1 : r3 = l1;
                           1'b0 : r3 = l3;
                        endcase
                        r4 = r1[5:5];
                        r5 = r3 & r4;
                        r6 = r1[4:0];
                        r7 = r1[5:5];
                        r8 = l4;
                        r8[4:0] = r6;
                        r9 = r8;
                        r9[5:5] = r7;
                        r10 = r9;
                        r10[6:6] = r5;
                        kernel_kernel = r10;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = Xfer::<b4>::default();
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
        let uut = Xfer::<b4>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_xfer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "49de8d04b4937bdc676a9597d84038e3040031cd3de5959d4c96a50ef775729e"
        ];
        let digest = vcd.dump_to_file(root.join("stream_xfer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> Result<(), RHDLError> {
        let a_rng = XorShift128::default()
            .map(|x| b4((x & 0xF) as u128))
            .take(10);
        let b_rng = a_rng.clone();
        let a_rng = stalling(a_rng, 0.23);
        let (sink, delivered) = SinkFromFn::new_from_iter_counted(b_rng, 0.3);
        let uut = TestFixture {
            source: SourceFromFn::new(a_rng),
            count: DFF::default(),
            xfer: Xfer::default(),
            sink,
        };
        let input = repeat_n((), 1000).with_reset(1).clock_pos_edge(100);
        let last_output = uut.run(input).last().unwrap();
        let last_count = last_output.output.raw();
        assert_eq!(last_count, 10);
        // `last_count` already counts transfers via the widget's own
        // `run` pulse, so this test was not vacuous. Pinning the sink
        // side too confirms the counted transfers actually *arrived*
        // rather than merely being counted.
        delivered.assert_at_least(10);
        Ok(())
    }
}
