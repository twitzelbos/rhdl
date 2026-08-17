//! Flatten Core
//!
//!# Purpose
//!
//! A [Flatten] Core takes a sequence of arrays of
//! type `[T; N]` and splits them into individual items of
//! type `T`.  It is roughly equivalent to calling
//! `.iter().flatten()` on an iterator that returns `[T; N]` slices.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [FlattenPipe] buffer
//!
#![doc = badascii_formal!("
         +-+FlattenPipe+--+        
 ?[T;N]  |                |  ?T    
+------->+ data     data  +------->
         |                |        
 R<[T;N]>|                | R<T>       
<--------+ ready    ready |<------+
         |                |        
         +----------------+       
")]
//!
//!# Internals
//!
//! The [Flatten] core uses a loadable delay line to hold the array in a
//! set of chained flip flops.  The output is then clocked off the end
//! of the chain, one element at a time.  When it is empty, the delay
//! line can be reloaded from the input buffer.  Buffers at the input
//! and output eliminate combinatorial paths.  This design is a bit
//! register/flip flop heavy, so be careful with its use.
//!
#![doc = badascii!(r"
        +IBuf+-----+        +-+unpck++                                                        
 ?[T;N] |          | ?[T;N] |        | [T;N]                                                  
+------>|data  data+------->|in  data+-------+---+---+                                        
R<[T;N]>|          |        |        |       v   v   v        +--+pck+-+    +OBuf+-----+      
<-------+ready next|<----+  |     tag+-+   +------------+     |        | ?T |          | ?T   
        |          |     |  |        | |   | Delay Line +---->|data out+--->|data  data+----->
        +----------+     |  +--------+ |   |      run   |     |        |    |          | R<T>      
                         |             v   +------------+ +-->|tag     | +--+full ready|<----+
                         |  +-----------+          ^      |   |        | |  |          |      
                         |  |   Control +----------+      |   +--------+ |  +----------+      
                         +--+           +-----------------+              |                    
                            |           |<-------------------------------+                    
                            +-----------+                                                     
")]
//!
//! The control is governed by a simple two-state state machine.  The state diagram
//! is as follows:
#![doc = badascii!(r"
                           +---------+                          
                           |         |                          
   !full && cnt == N-1     | Loading |                          
     && !in_is_some     +->|         +--+    in_is_some         
   +----------------+  /   +---------+   \  +-----------+       
       cnt = 1        +                   +   next = 1          
       load = 0       |                   |   cnt = 0           
                      +                   +   load = 1          
                       \   +---------+   /                      
!full && cnt == N-1     +--+         |<-+                       
     && in_is_some         | Running |                          
+------------------+  +--->|         +-----+                    
    next = 1          |    |         |     | !full && cnt != N-1
    cnt = 0           |    |         |     | +-----------------+
    load = 1          |    |         |<----+    run = 1         
                      +----+         |          cnt += 1        
                           +-------+-+                          
                              ^    |                            
                              +----+                            
                               full                             
                              +----+                            
                               run=0                            
                               load=0                           
")]
//!# Example
//!
//! Here is an example of running the pipelined reducer.
//!
//!```
#![doc = include_str!("../../examples/flatten.rs")]
//!```
//!
//! with a trace file like this:
//!
#![doc = include_str!("../../doc/flatten.md")]
//!
use crate::{
    core::{dff, option::is_some},
    stream::{fifo_to_stream::FIFOToStream, stream_to_fifo::StreamToFIFO},
};

use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use super::StreamIO;

#[derive(Debug, Default, PartialEq, Digital, Clone, Copy)]
#[doc(hidden)]
pub enum State {
    #[default]
    Loading,
    Running,
}

#[derive(Debug, Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The [Flatten] Core
///
/// This core takes a stream of `[T; N]`, and produces
/// a stream of `T`, reading out the input stream in
/// index order (`0, 1, 2..`).  
pub struct Flatten<T: Digital, const M: usize, const N: usize>
where
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    input_buffer: StreamToFIFO<[T; N]>,
    delay: [dff::DFF<T>; N],
    count: dff::DFF<Bits<M>>,
    output_buffer: FIFOToStream<T>,
    state: dff::DFF<State>,
}

impl<T: Digital, const M: usize, const N: usize> Default for Flatten<T, M, N>
where
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    fn default() -> Self {
        assert!(
            (1 << M) >= N,
            "Expect that the bitwidth of the counter is sufficient to count the elements in the array.  I.e., (1 << M) >= N"
        );
        Self {
            delay: core::array::from_fn(|_| dff::DFF::new(T::dont_care())),
            input_buffer: StreamToFIFO::default(),
            count: dff::DFF::new(bits(0)),
            output_buffer: FIFOToStream::default(),
            state: dff::DFF::new(State::Loading),
        }
    }
}

/// Inputs for the [FlattenPipe] core
pub type In<T, const N: usize> = StreamIO<[T; N], T>;

/// Outputs from the [FlattenPipe] core
pub type Out<T, const N: usize> = StreamIO<T, [T; N]>;

impl<T: Digital, const M: usize, const N: usize> SynchronousIO for Flatten<T, M, N>
where
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    type I = In<T, N>;
    type O = Out<T, N>;
    type Kernel = kernel<T, M, N>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<T: Digital, const M: usize, const N: usize>(
    _cr: ClockReset,
    i: In<T, N>,
    q: Q<T, M, N>,
) -> (Out<T, N>, D<T, M, N>)
where
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    let n_minus_1 = bits::<M>(N as u128 - 1);
    let mut d = D::<T, M, N>::dont_care();
    // Connect the input buffer to the input data stream
    d.input_buffer.data = i.data;
    // Do not advance the input buffer unless asked.
    d.input_buffer.next = false;
    // Control line to load the delay line from the
    // input buffer
    let mut load_line = false;
    // Control line to write the delay line output
    // to the output buffer (also advances the delay line)
    let mut write = false;
    // By default, do not change the count or state
    d.count = q.count;
    d.state = q.state;
    let out_full = q.output_buffer.full;
    let in_some = is_some::<[T; N]>(q.input_buffer.data);
    // Update the state and compute transition actions
    match q.state {
        State::Loading => {
            if in_some {
                // Accept the input data
                d.input_buffer.next = true;
                // Load the data into the delay line
                load_line = true;
                // Reset the counter
                d.count = bits(0);
                d.state = State::Running;
            }
        }
        State::Running => {
            if !out_full {
                write = true;
                if q.count != n_minus_1 {
                    d.count = q.count + 1;
                } else if in_some {
                    // Finished, and on this write, we
                    // will load the next data (which is available)
                    d.input_buffer.next = true;
                    d.count = bits(0);
                    load_line = true;
                } else {
                    // No more data.  Go back to Loading
                    d.state = State::Loading;
                }
            }
        }
    }
    // By default, the delay line holds it's current
    // state
    for i in 0..N {
        d.delay[i] = q.delay[i];
    }
    if write {
        // The write signal indicates the delay line should
        // shift
        for i in 1..N {
            d.delay[i - 1] = q.delay[i]
        }
    }
    if load_line {
        if let Some(idata) = q.input_buffer.data {
            // Reload the delay line from the input buffer
            for i in 0..N {
                d.delay[i] = idata[i]
            }
        }
    }
    // Use the write flag to strobe data into the output FIFO
    d.output_buffer.data = if write { Some(q.delay[0]) } else { None };
    d.output_buffer.ready = i.ready;
    let o = Out::<T, N> {
        data: q.output_buffer.data,
        ready: q.input_buffer.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {

    /// A data-gated sink must not stall the expander.
    ///
    /// `Flatten` holds an array while emitting its elements, so it
    /// presents `None` between groups while still needing to accept the
    /// next array — the "absorb without emitting" shape that deadlocked
    /// `stream::filter`. Safe here because the input is pulled with an
    /// explicit `next`; pinned down rather than left inferred.
    #[test]
    fn data_gated_sink_does_not_stall_the_expander() -> Result<(), RHDLError> {
        use crate::stream::StreamIO;
        use rhdl::core::sim::ResetOrData;

        const GROUPS: u128 = 5;
        let uut = Flatten::<b4, 2, 4>::default();
        let mut sent: u128 = 0;
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
                let mut input = StreamIO::<[b4; 4], b4> {
                    data: None,
                    ready: ready::<b4>(sink_ready),
                };
                if sent < GROUPS && output.ready.raw {
                    let b = sent * 4;
                    input.data = Some([
                        b4(b % 16),
                        b4((b + 1) % 16),
                        b4((b + 2) % 16),
                        b4((b + 3) % 16),
                    ]);
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);

        assert_eq!(sent, GROUPS, "the source must not be stalled forever");
        let want: Vec<u128> = (0..GROUPS)
            .flat_map(|g| (0..4u128).map(move |e| (g * 4 + e) % 16))
            .collect();
        assert_eq!(
            got, want,
            "every element of every group must arrive, in order"
        );
        Ok(())
    }

    use crate::{rng::xorshift::XorShift128, stream::ready};

    use super::*;

    fn mk_array<T, const N: usize>(t: &mut impl Iterator<Item = T>) -> [T; N] {
        core::array::from_fn(|_| t.next().unwrap())
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Flatten::<b4, 2, 4>::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// `Flatten` holds an array while emitting its elements one at a
    /// time, so it presents `None` to its *input* between groups.
    /// Offers gapped on 4, `ready` withheld on 3 — coprime, so the
    /// stall lands at different positions within a group rather than
    /// always at the same element.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4, 4>)>> {
        (0..32u128)
            .map(|k| In::<b4, 4> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some([
                        b4(k % 16),
                        b4((k + 1) % 16),
                        b4((k + 2) % 16),
                        b4((k + 3) % 16),
                    ])
                },
                ready: crate::stream::ready::<b4>(!k.is_multiple_of(3)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Flatten::<b4, 2, 4>::default();
        let desc = uut.descriptor("stream_flatten".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_flatten")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_flatten(input wire [1:0] clock_reset, input wire [17:0] i, output wire [5:0] o);
               wire [48:0] od;
               wire [42:0] d;
               wire [44:0] q;
               assign o = od[5:0];
               stream_flatten_input_buffer c0(.clock_reset(clock_reset), .i(d[17:0]), .o(q[18:0]));
               stream_flatten_delay c1(.clock_reset(clock_reset), .i(d[33:18]), .o(q[34:19]));
               stream_flatten_count c2(.clock_reset(clock_reset), .i(d[35:34]), .o(q[36:35]));
               stream_flatten_output_buffer c3(.clock_reset(clock_reset), .i(d[41:36]), .o(q[43:37]));
               stream_flatten_state c4(.clock_reset(clock_reset), .i(d[42:42]), .o(q[44:44]));
               assign d = od[48:6];
               assign od = kernel_kernel(clock_reset, i, q);
               function [48:0] kernel_kernel(input reg [1:0] arg_0, input reg [17:0] arg_1, input reg [44:0] arg_2);
                     reg [16:0] r0;
                     reg [17:0] r1;
                     // d
                     reg [42:0] r2;
                     // d
                     reg [42:0] r3;
                     reg [1:0] r4;
                     reg [44:0] r5;
                     // d
                     reg [42:0] r6;
                     reg [0:0] r7;
                     // d
                     reg [42:0] r8;
                     reg [6:0] r9;
                     reg [0:0] r10;
                     reg [18:0] r11;
                     reg [16:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     // d
                     reg [42:0] r16;
                     // d
                     reg [42:0] r17;
                     // d
                     reg [42:0] r18;
                     // d
                     reg [42:0] r19;
                     // load_line
                     reg [0:0] r20;
                     reg [0:0] r21;
                     reg [1:0] r22;
                     reg [0:0] r23;
                     reg [1:0] r24;
                     reg [1:0] r25;
                     // d
                     reg [42:0] r26;
                     // d
                     reg [42:0] r27;
                     // d
                     reg [42:0] r28;
                     // d
                     reg [42:0] r29;
                     // d
                     reg [42:0] r30;
                     // load_line
                     reg [0:0] r31;
                     // d
                     reg [42:0] r32;
                     // load_line
                     reg [0:0] r33;
                     // d
                     reg [42:0] r34;
                     // load_line
                     reg [0:0] r35;
                     // write
                     reg [0:0] r36;
                     // d
                     reg [42:0] r37;
                     // load_line
                     reg [0:0] r38;
                     // write
                     reg [0:0] r39;
                     reg [15:0] r40;
                     reg [3:0] r41;
                     // d
                     reg [42:0] r42;
                     reg [15:0] r43;
                     reg [3:0] r44;
                     // d
                     reg [42:0] r45;
                     reg [15:0] r46;
                     reg [3:0] r47;
                     // d
                     reg [42:0] r48;
                     reg [15:0] r49;
                     reg [3:0] r50;
                     // d
                     reg [42:0] r51;
                     reg [15:0] r52;
                     reg [3:0] r53;
                     // d
                     reg [42:0] r54;
                     reg [15:0] r55;
                     reg [3:0] r56;
                     // d
                     reg [42:0] r57;
                     reg [15:0] r58;
                     reg [3:0] r59;
                     // d
                     reg [42:0] r60;
                     // d
                     reg [42:0] r61;
                     reg [18:0] r62;
                     reg [16:0] r63;
                     reg [0:0] r64;
                     reg [15:0] r65;
                     reg [3:0] r66;
                     // d
                     reg [42:0] r67;
                     reg [3:0] r68;
                     // d
                     reg [42:0] r69;
                     reg [3:0] r70;
                     // d
                     reg [42:0] r71;
                     reg [3:0] r72;
                     // d
                     reg [42:0] r73;
                     // d
                     reg [42:0] r74;
                     // d
                     reg [42:0] r75;
                     reg [15:0] r76;
                     reg [3:0] r77;
                     reg [4:0] r78;
                     reg [3:0] r79;
                     reg [4:0] r80;
                     // d
                     reg [42:0] r81;
                     reg [0:0] r82;
                     // d
                     reg [42:0] r83;
                     reg [6:0] r84;
                     reg [4:0] r85;
                     reg [18:0] r86;
                     reg [0:0] r87;
                     reg [5:0] r88;
                     reg [5:0] r89;
                     reg [48:0] r90;
                     reg [1:0] r91;
                     localparam l0 = 43'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b0;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b0;
                     localparam l5 = 1'b0;
                     localparam l6 = 1'b1;
                     localparam l7 = 2'b00;
                     localparam l8 = 1'b1;
                     localparam l9 = 1'b1;
                     localparam l10 = 1'b0;
                     localparam l11 = 2'b11;
                     localparam l12 = 2'b01;
                     localparam l13 = 1'b1;
                     localparam l14 = 2'b00;
                     localparam l15 = 1'b0;
                     localparam l16 = 1'b1;
                     localparam l17 = 1'b1;
                     localparam l18 = 1'b0;
                     localparam l19 = 1'b0;
                     localparam l20 = 1'b1;
                     localparam l21 = 1'b1;
                     localparam l22 = 1'b1;
                     localparam l23 = 5'b00000;
                     localparam l24 = 6'b000000;
                     begin
                        r91 = arg_0;
                        r1 = arg_1;
                        r5 = arg_2;
                        r0 = r1[16:0];
                        r2 = l0;
                        r2[16:0] = r0;
                        r3 = r2;
                        r3[17:17] = l1;
                        r4 = r5[36:35];
                        r6 = r3;
                        r6[35:34] = r4;
                        r7 = r5[44:44];
                        r8 = r6;
                        r8[42:42] = r7;
                        r9 = r5[43:37];
                        r10 = r9[5:5];
                        r11 = r5[18:0];
                        r12 = r11[16:0];
                        r13 = r12[16:16];
                        case (r13)
                           1'b1 : r14 = l3;
                           1'b0 : r14 = l5;
                        endcase
                        r15 = r5[44:44];
                        r16 = r8;
                        r16[17:17] = l6;
                        r17 = r16;
                        r17[35:34] = l7;
                        r18 = r17;
                        r18[42:42] = l8;
                        r19 = r14 ? r18 : r8;
                        r20 = r14 ? l9 : l10;
                        r21 = ~r10;
                        r22 = r5[36:35];
                        r23 = r22 != l11;
                        r24 = r5[36:35];
                        r25 = r24 + l12;
                        r26 = r8;
                        r26[35:34] = r25;
                        r27 = r8;
                        r27[17:17] = l13;
                        r28 = r27;
                        r28[35:34] = l14;
                        r29 = r8;
                        r29[42:42] = l15;
                        r30 = r14 ? r28 : r29;
                        r31 = r14 ? l16 : l10;
                        r32 = r23 ? r26 : r30;
                        r33 = r23 ? l10 : r31;
                        r34 = r21 ? r32 : r8;
                        r35 = r21 ? r33 : l10;
                        r36 = r21 ? l17 : l18;
                        case (r15)
                           1'b0 : r37 = r19;
                           1'b1 : r37 = r34;
                        endcase
                        case (r15)
                           1'b0 : r38 = r20;
                           1'b1 : r38 = r35;
                        endcase
                        case (r15)
                           1'b0 : r39 = l18;
                           1'b1 : r39 = r36;
                        endcase
                        r40 = r5[34:19];
                        r41 = r40[3:0];
                        r42 = r37;
                        r42[21:18] = r41;
                        r43 = r5[34:19];
                        r44 = r43[7:4];
                        r45 = r42;
                        r45[25:22] = r44;
                        r46 = r5[34:19];
                        r47 = r46[11:8];
                        r48 = r45;
                        r48[29:26] = r47;
                        r49 = r5[34:19];
                        r50 = r49[15:12];
                        r51 = r48;
                        r51[33:30] = r50;
                        r52 = r5[34:19];
                        r53 = r52[7:4];
                        r54 = r51;
                        r54[21:18] = r53;
                        r55 = r5[34:19];
                        r56 = r55[11:8];
                        r57 = r54;
                        r57[25:22] = r56;
                        r58 = r5[34:19];
                        r59 = r58[15:12];
                        r60 = r57;
                        r60[29:26] = r59;
                        r61 = r39 ? r60 : r51;
                        r62 = r5[18:0];
                        r63 = r62[16:0];
                        r64 = r63[16:16];
                        r65 = r63[15:0];
                        r66 = r65[3:0];
                        r67 = r61;
                        r67[21:18] = r66;
                        r68 = r65[7:4];
                        r69 = r67;
                        r69[25:22] = r68;
                        r70 = r65[11:8];
                        r71 = r69;
                        r71[29:26] = r70;
                        r72 = r65[15:12];
                        r73 = r71;
                        r73[33:30] = r72;
                        case (r64)
                           1'b1 : r74 = r73;
                           default : r74 = r61;
                        endcase
                        r75 = r38 ? r74 : r61;
                        r76 = r5[34:19];
                        r77 = r76[3:0];
                        r79 = r77[3:0];
                        r78 = {l22, r79};
                        r80 = r39 ? r78 : l23;
                        r81 = r75;
                        r81[40:36] = r80;
                        r82 = r1[17:17];
                        r83 = r81;
                        r83[41:41] = r82;
                        r84 = r5[43:37];
                        r85 = r84[4:0];
                        r86 = r5[18:0];
                        r87 = r86[17:17];
                        r88 = l24;
                        r88[4:0] = r85;
                        r89 = r88;
                        r89[5:5] = r87;
                        r90 = {r83, r89};
                        kernel_kernel = r90;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = Flatten::<b4, 2, 4>::default();
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
        let uut = Flatten::<b4, 2, 4>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_flatten");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "e02140220462767f69da53f6cd84e6fc552120a5c2de0565e0b388fd12903705"
        ];
        let digest = vcd.dump_to_file(root.join("stream_flatten.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> miette::Result<()> {
        type Uut = Flatten<b4, 2, 4>;
        let uut = Uut::default();
        let mut need_reset = true;
        let mut source_rng = XorShift128::default().map(|x| bits((x & 0xF) as u128));
        let mut dest_rng = source_rng.clone();
        let mut latched_input: Option<[b4; 4]> = None;
        uut.run_fn(
            move |out| {
                if need_reset {
                    need_reset = false;
                    return Some(rhdl::core::sim::ResetOrData::Reset);
                }
                let mut input = super::In::<b4, 4>::dont_care();
                // Downstream is likely to run
                let want_to_pause = rand::random::<u8>() > 200;
                input.ready = ready(!want_to_pause);
                // Decide if the producer will generate a new data item
                let willing_to_send = rand::random::<u8>() < 200;
                if out.ready.raw {
                    // The pipeline wants more data
                    if willing_to_send {
                        latched_input = Some(mk_array(&mut source_rng));
                    } else {
                        latched_input = None;
                    }
                }
                input.data = latched_input;
                if input.ready.raw && out.data.is_some() {
                    assert_eq!(dest_rng.next(), out.data);
                }
                Some(rhdl::core::sim::ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 100_000)
        .for_each(drop);
        Ok(())
    }
}
