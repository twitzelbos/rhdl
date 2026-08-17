//!# Carloni Buffer with Option Interface
//!
//! This core wraps the [Carloni] skid buffer core
//! in a more ergonomic [Option] based interface.  The
//! `void_in` input and the `data_in` are combined into
//! a single `data_in` with type [Option<T>], and the
//! `void_out` and `data_out` are similarly combined
//! into a single `data_out` with type [Option<T>].  Furthermore
//! for compatibility with the `ready-valid` interface used
//! elsewhere in RHDL, the `stop` signals are inverted to be
//! `valid` signals.
//!
//!# Schematic symbol
//!
//! Here is the symbol for the buffer.
//!
#![doc = badascii_formal!("
        +-+OptionCarloni++      
    ?T  |                | ?T   
   +--->|data        data+----> 
Ready<T>|                | Ready<T>     
   <----+ready      ready|<----+
        |                |      
        +----------------+      
")]
//!
//!# Internal details
//!
//! Internally, the buffer is simply a [CarloniBuffer]
//! with `pack` and `unpack` cores to convert the
//! [Option<T>]  to a pair of `data` and `valid` lines.
//! The code is pretty short and self-expanatory.
//!
//! Here is a sketch of the internals
//!
#![doc = badascii!(r"
                            +-----+Carloni+-------+                          
      ++unpck++             |                     |           ++pack+-+      
 data |       |  +--------->| data_in    data_out +-------+   |       | data 
+---->|in    T+--+          |                     |       +-->|T   out+----->
      |       |        +----+ stop_out   stop_in  |<---+      |       |      
      |  valid+--+     |    |                     |    |      |       |      
      |       |  +-----+--->| void_in    void_out +----+----->|valid  |      
      +-------+        |    |                     |    |      +-------+      
                       |    +---------------------+    |                     
          +            |                               |        +            
 ready   /|            |                               |       /|  ready     
<-----+○+ |<-----------+                               +----+○+ |<------+    
         \|                                                    \|            
          +                                                     +            
")]
//!
//!# Example
//!
//! Here is the example from the [CarloniBuffer] with an [Option<T>]
//! based interface.
//!
//!```
#![doc = include_str!("../../examples/stream_buffer.rs")]
//!```
//!
//! With trace
//!
#![doc = include_str!("../../doc/option_carloni.md")]
//!
use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::{
    core::option::pack,
    lid::carloni::Carloni,
    stream::{StreamIO, ready},
};

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// Option-based Carloni buffer core
///
/// Here `T` is the data type being transported through
/// the buffer.
pub struct StreamBuffer<T: Digital> {
    inner: Carloni<T>,
}

impl<T: Digital> Default for StreamBuffer<T> {
    fn default() -> Self {
        Self {
            inner: Carloni::default(),
        }
    }
}

/// Inputs to the [StreamBuffer] buffer core
pub type In<T> = StreamIO<T, T>;

/// Outputs from the [StreamBuffer] buffer core
pub type Out<T> = StreamIO<T, T>;

impl<T: Digital> SynchronousIO for StreamBuffer<T> {
    type I = In<T>;
    type O = Out<T>;
    type Kernel = option_carloni_kernel<T>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn option_carloni_kernel<T: Digital>(_cr: ClockReset, i: In<T>, q: Q<T>) -> (Out<T>, D<T>) {
    let mut d = D::<T>::dont_care();
    let (data_valid, data) = match i.data {
        Some(data) => (true, data),
        None => (false, T::dont_care()),
    };
    d.inner.data_in = data;
    d.inner.void_in = !data_valid;
    d.inner.stop_in = !i.ready.raw;
    let mut o = Out::<T>::dont_care();
    o.ready = ready::<T>(!q.inner.stop_out);
    o.data = pack::<T>(!q.inner.void_out, q.inner.data_out);
    (o, d)
}

#[cfg(test)]
mod tests {

    /// A data-gated sink — one that withholds `ready` whenever it sees
    /// nothing on the wire — must not stall this buffer.
    ///
    /// `StreamBuffer` is the skid primitive underneath `map`, `filter`
    /// and `filter_map`, so if it mishandled that (legal) sink shape,
    /// everything built on it would inherit the fault. `filter`'s
    /// deadlock turned out to be in `filter` itself rather than here —
    /// this pins that down instead of leaving it inferred.
    #[test]
    fn data_gated_sink_does_not_stall_the_buffer() -> Result<(), RHDLError> {
        use crate::stream::{StreamIO, ready};
        use rhdl::core::sim::ResetOrData;

        const COUNT: u128 = 16;
        let uut = StreamBuffer::<b4>::default();
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
                if let Some(d) = output.data {
                    got.push(d.raw());
                }
                let mut input = StreamIO::<b4, b4> {
                    data: None,
                    ready: ready::<b4>(sink_ready),
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

        assert_eq!(to_send, COUNT, "the source must not be stalled forever");
        let want: Vec<u128> = (0..COUNT).collect();
        assert_eq!(got, want, "every item must survive a data-gated sink");
        Ok(())
    }
    use rhdl::core::sim::ResetOrData;

    use crate::rng::xorshift::XorShift128;

    use super::*;

    /// Open-loop stimulus for Tiers 3-5: offers gapped on 4, `ready`
    /// withheld on 3. Coprime so they drift and the skid path — hold an
    /// item while the sink stalls — is actually entered.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4>)>> {
        (0..24u128)
            .map(|k| In::<b4> {
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
        let uut = StreamBuffer::<b4>::default();
        let desc = uut.descriptor("stream_buffer".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_buffer")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_buffer(input wire [1:0] clock_reset, input wire [5:0] i, output wire [5:0] o);
               wire [11:0] od;
               wire [5:0] d;
               wire [5:0] q;
               assign o = od[5:0];
               stream_buffer_inner c0(.clock_reset(clock_reset), .i(d[5:0]), .o(q[5:0]));
               assign d = od[11:6];
               assign od = kernel_option_carloni_kernel(clock_reset, i, q);
               function [11:0] kernel_option_carloni_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [5:0] arg_2);
                     reg [4:0] r0;
                     reg [5:0] r1;
                     reg [0:0] r2;
                     reg [3:0] r3;
                     reg [4:0] r4;
                     reg [4:0] r5;
                     reg [0:0] r6;
                     reg [3:0] r7;
                     // d
                     reg [5:0] r8;
                     reg [0:0] r9;
                     // d
                     reg [5:0] r10;
                     reg [0:0] r11;
                     reg [0:0] r12;
                     // d
                     reg [5:0] r13;
                     reg [5:0] r14;
                     reg [0:0] r15;
                     reg [0:0] r16;
                     reg [0:0] r17;
                     reg [0:0] r18;
                     // o
                     reg [5:0] r19;
                     reg [0:0] r20;
                     reg [0:0] r21;
                     reg [3:0] r22;
                     reg [4:0] r23;
                     reg [3:0] r24;
                     reg [4:0] r25;
                     // o
                     reg [5:0] r26;
                     reg [11:0] r27;
                     reg [1:0] r28;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 5'bXXXX0;
                     localparam l4 = 6'bXXXXXX;
                     localparam l5 = 1'b0;
                     localparam l6 = 6'bXXXXXX;
                     localparam l7 = 1'b1;
                     localparam l8 = 5'b00000;
                     begin
                        r28 = arg_0;
                        r1 = arg_1;
                        r14 = arg_2;
                        r0 = r1[4:0];
                        r2 = r0[4:4];
                        r3 = r0[3:0];
                        r4 = {r3, l0};
                        case (r2)
                           1'b1 : r5 = r4;
                           1'b0 : r5 = l3;
                        endcase
                        r6 = r5[0:0];
                        r7 = r5[4:1];
                        r8 = l4;
                        r8[3:0] = r7;
                        r9 = ~r6;
                        r10 = r8;
                        r10[4:4] = r9;
                        r11 = r1[5:5];
                        r12 = ~r11;
                        r13 = r10;
                        r13[5:5] = r12;
                        r15 = r14[5:5];
                        r16 = ~r15;
                        r17 = l5;
                        r18 = r17;
                        r18[0:0] = r16;
                        r19 = l6;
                        r19[5:5] = r18;
                        r20 = r14[4:4];
                        r21 = ~r20;
                        r22 = r14[3:0];
                        r24 = r22[3:0];
                        r23 = {l7, r24};
                        r25 = r21 ? r23 : l8;
                        r26 = r19;
                        r26[4:0] = r25;
                        r27 = {r13, r26};
                        kernel_option_carloni_kernel = r27;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = StreamBuffer::<b4>::default();
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
        let uut = StreamBuffer::<b4>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_buffer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "678bfc13d3c98ee382019f9df6acd3a6a6157e6a05f91119093b761ea49a9bfc"
        ];
        let digest = vcd.dump_to_file(root.join("stream_buffer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_option_carloni_buffer() {
        let uut = StreamBuffer::<b32>::default();
        let mut need_reset = true;
        let mut source_rng = XorShift128::default();
        let mut output_rng = XorShift128::default();
        uut.run_fn(
            |out| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                let mut input = In::<b32>::dont_care();
                // Downstream reandomly wants to pause
                let want_to_pause = rand::random::<u8>() > 200;
                input.ready = ready(!want_to_pause);
                // Upstream may have paused
                let want_to_send = rand::random::<u8>() < 200;
                input.data = None;
                if out.ready.raw && want_to_send {
                    // The receiver did not tell us to stop, and
                    // we want to send something
                    input.data = Some(bits(source_rng.next().unwrap() as u128));
                }
                // Check output
                if out.data.is_some() && input.ready.raw {
                    // The output will advance on this clock cycle
                    assert_eq!(out.data, Some(bits(output_rng.next().unwrap() as u128)));
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 100_000)
        .for_each(drop);
    }
}
