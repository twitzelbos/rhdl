//! Chunked Stream Core
//!
//!# Purpose
//!
//! A [Chunked] Stream Core takes a sequence of `T` data elements
//! and chunks them into an array of `N`.  It is roughly equivalent
//! to the `.chunks()` method on slices.  Note that each chunk
//! will contain a disjoint set of samples.  
//!
#![doc = badascii!(r"
      t0  t1  t2  t3  t4  t5  t6  t7  t8 ...
                                            
 in   d0  d1  d2  d3  d4  d5  d6  d7  d8    
                                            
out               [d0..d3]         [d4..d7] 
")]
//! If you want a sliding window, use the [WindowedPipe] Core instead.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [ChunkedPipe] core.
//!
#![doc = badascii_formal!("
     ++Chunked+-----+        
 ?T  |              | ?[T;N] 
+--->|data      data+------->
 R<T>|              | R<[T;N]>       
<----+ready    ready|<------+
     |              |        
     +--------------+        
")]
//!
//!# Internals
//!
//! Roughly, the internal of the [Chunked] core includes
//! a pipeline delay stage, along with taps to extract the
//! delayed signals.  Buffers are needed at the input and
//! output to isolate the combinatorial signals from each other.
//!
//! Note, in particular, that the `run` signal depends on the validity
//! of the input `data` element.  Without an input buffer, we would have
//! a combinatorial path between the input and output.  
//!
#![doc = badascii!(r"
                      ++unpck+-+    ++TappedDelay+---+ [T;N]  ++pck++      ++Fifo2St+-+      
     ++St2Fifo+-+     |        |  T |             out+------->|data |?[T;N]|          |      
 ?T  |          | ?T  |    data+--->|in     run      |        |  out+----->|data  data+----->
+--->|data  data+---->|in      |    +----------------+   +--->|tag  |      |          |      
     |          |     |     tag+--+          ^           |    |     |   +--+full ready|<----+
<----+ready next|<-+  |        |  |   +------+-----+     |    +-----+   |  |          |      
     |          |  |  +--------+  +-->|  Control   +-----+              |  +----------+      
     +----------+  |         =run     +-+----------+                    |                    
                   +--------------------+      ^                        |                    
                                               +------------------------+                    
")]
//!
//! The two pipelines (upstream and downstream) are connected with the buffered
//! tapped-delay line.  The control system is a simple two-state state machine.
#![doc = badascii!("
                     +------+                                  
           !in_some  |      v   +-------+ in_some && cnt != N-1
                     |    +-----+-+     | +-------------------+
                     +----+Loading|<----+   run = 1, cnt += 1  
                       +->|       |                            
in_some && !out_full   |  +-----+-+ in_some && cnt == N-1      
+------------------+   |        |   +-------------------+      
 cnt = 1, run = 1      |        |      run = 1, cnt = 0        
  do_write = 1         |        v                              
                       |     +----+                            
!in_some && !out_full  +-----+Full+--+                         
+--------------------+       +----+  |                         
 cnt = 0, run = 0               ^    | out_full                
  do_write = 1                  +----+                         
")]
//!
//!# Example
//!
//! The example includes some of the testing tools for generating and
//! sinking Pipe cores.  These are not synthesizable, but are handy for
//! testing and verification exercises.
//!
//!```
#![doc = include_str!("../../examples/chunk.rs")]
//!```
//!
//! The output trace demonstrates the core in action.
#![doc = include_str!("../../doc/chunk.md")]

use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::{
    core::dff,
    stream::{fifo_to_stream, stream_to_fifo},
};

use super::StreamIO;

#[derive(Debug, Default, PartialEq, Digital, Clone, Copy)]
#[doc(hidden)]
pub enum State {
    #[default]
    Loading,
    Full,
}

#[derive(Debug, Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// The Chunked Stream Core
///
/// This core takes a stream of `T` and produces
/// a stream of chunks `[T;N]`, assembling the array
/// in index order, so that `t0, t1, t2,...` are
/// packed such that the `out[0] = t0`, etc.
/// Note that `M` is a bitwidth for the internal counter
/// and must satisfy `1 << M <= N`.
pub struct Chunked<T: Digital, const M: usize, const N: usize>
where
    [T; N]: Default,
    T: Default,
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    input_buffer: stream_to_fifo::StreamToFIFO<T>,
    delay_line: [dff::DFF<T>; N],
    count: dff::DFF<Bits<M>>,
    output_buffer: fifo_to_stream::FIFOToStream<[T; N]>,
    state: dff::DFF<State>,
}

impl<T: Digital, const M: usize, const N: usize> Default for Chunked<T, M, N>
where
    [T; N]: Default,
    T: Default,
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    fn default() -> Self {
        assert!(N > 1, "Can only chunk streams with N > 1");
        assert!(
            (1 << M) >= N,
            "Expect that the bitwidth of the counter is sufficiently large to express values up to N"
        );
        Self {
            input_buffer: stream_to_fifo::StreamToFIFO::default(),
            delay_line: core::array::from_fn(|_| dff::DFF::default()),
            count: dff::DFF::new(bits(0)),
            output_buffer: fifo_to_stream::FIFOToStream::default(),
            state: dff::DFF::new(State::Loading),
        }
    }
}

/// Inputs for the [Chunked] core
pub type In<T, const N: usize> = StreamIO<T, [T; N]>;

/// Outputs from the [Chunked] core
pub type Out<T, const N: usize> = StreamIO<[T; N], T>;

impl<T: Digital, const M: usize, const N: usize> SynchronousIO for Chunked<T, M, N>
where
    [T; N]: Default,
    T: Default,
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    type I = In<T, N>;
    type O = Out<T, N>;
    type Kernel = kernel<T, M, N>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<T, const M: usize, const N: usize>(
    _cr: ClockReset,
    i: In<T, N>,
    q: Q<T, M, N>,
) -> (Out<T, N>, D<T, M, N>)
where
    [T; N]: Default,
    T: Default + Digital,
    rhdl::bits::W<M>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    let n_minus_1 = bits::<M>(N as u128 - 1);
    let mut d = D::<T, M, N>::dont_care();
    d.input_buffer.data = i.data;
    d.output_buffer.ready = i.ready;
    let mut write = false;
    let mut run = false;
    d.count = q.count;
    d.state = q.state;
    let out_full = q.output_buffer.full;
    // Update the state and compute transition actions
    d.delay_line[0] = q.delay_line[0];
    match q.state {
        State::Loading => {
            if let Some(idata) = q.input_buffer.data {
                if q.count != n_minus_1 {
                    run = true;
                    d.count = q.count + 1;
                } else {
                    run = true;
                    d.state = State::Full;
                }
                d.delay_line[0] = idata;
            }
        }
        State::Full => {
            if !out_full {
                write = true;
                d.state = State::Loading;
                if let Some(idata) = q.input_buffer.data {
                    d.count = bits(1);
                    d.delay_line[0] = idata;
                    run = true;
                } else {
                    d.count = bits(0);
                }
            }
        }
    }
    // Implement the delay line
    for i in 1..N {
        d.delay_line[i] = if run {
            q.delay_line[i - 1]
        } else {
            q.delay_line[i]
        }
    }
    // Feed the run signal to the input buffer
    d.input_buffer.next = run;
    // Feed the tapped delay line output to the
    // output buffer, modulated by the write signal
    d.output_buffer.data = if write {
        let mut tmp = <[T; N]>::dont_care();
        for i in 0..N {
            tmp[N - 1 - i] = q.delay_line[i]
        }
        Some(tmp)
    } else {
        None
    };
    let o = Out::<T, N> {
        data: q.output_buffer.data,
        ready: q.input_buffer.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {

    /// A data-gated sink must not stall the gatherer.
    ///
    /// `Chunked` absorbs `N` items and emits one, so it spends most of
    /// its time presenting `None` while still needing to accept — the
    /// same "absorb without emitting" shape that deadlocked
    /// `stream::filter`. It is safe because it pulls from its input
    /// buffer with an explicit `next` it controls rather than gating on
    /// the downstream `ready`; this test pins that down.
    #[test]
    fn data_gated_sink_does_not_stall_the_gatherer() -> Result<(), RHDLError> {
        use crate::stream::testing::closed_loop::assert_lossless_mapped;

        const COUNT: u128 = 16;
        let uut = Chunked::<b4, 2, 4>::default();
        let src: Vec<b4> = (0..COUNT).map(|k| b4(k % 16)).collect();
        let want: Vec<[b4; 4]> = (0..COUNT / 4)
            .map(|c| {
                let b = c * 4;
                [b4(b), b4(b + 1), b4(b + 2), b4(b + 3)]
            })
            .collect();
        assert_lossless_mapped(&uut, &src, &want);
        Ok(())
    }

    use crate::{rng::xorshift::XorShift128, stream::ready};

    use super::*;

    fn mk_array<T, const N: usize>(mut t: impl Iterator<Item = T>) -> impl Iterator<Item = [T; N]> {
        std::iter::from_fn(move || Some(core::array::from_fn(|_| t.next().unwrap())))
    }

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Chunked::<b4, 2, 4>::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// `Chunked` absorbs `N` items and emits one array, so it spends
    /// most of its time presenting `None` while still needing to accept.
    /// Offers are gapped on 4 and `ready` withheld on 3 — coprime, so
    /// the stall lands at varying points within a chunk rather than
    /// always on the same element.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4, 4>)>> {
        (0..32u128)
            .map(|k| In::<b4, 4> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b4(k % 16))
                },
                ready: crate::stream::ready::<[b4; 4]>(!k.is_multiple_of(3)),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Chunked::<b4, 2, 4>::default();
        let desc = uut.descriptor("stream_chunked".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_chunked")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_chunked(input wire [1:0] clock_reset, input wire [5:0] i, output wire [17:0] o);
               wire [60:0] od;
               wire [42:0] d;
               wire [44:0] q;
               assign o = od[17:0];
               stream_chunked_input_buffer c0(.clock_reset(clock_reset), .i(d[5:0]), .o(q[6:0]));
               stream_chunked_delay_line c1(.clock_reset(clock_reset), .i(d[21:6]), .o(q[22:7]));
               stream_chunked_count c2(.clock_reset(clock_reset), .i(d[23:22]), .o(q[24:23]));
               stream_chunked_output_buffer c3(.clock_reset(clock_reset), .i(d[41:24]), .o(q[43:25]));
               stream_chunked_state c4(.clock_reset(clock_reset), .i(d[42:42]), .o(q[44:44]));
               assign d = od[60:18];
               assign od = kernel_kernel(clock_reset, i, q);
               function [60:0] kernel_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [44:0] arg_2);
                     reg [4:0] r0;
                     reg [5:0] r1;
                     // d
                     reg [42:0] r2;
                     reg [0:0] r3;
                     // d
                     reg [42:0] r4;
                     reg [1:0] r5;
                     reg [44:0] r6;
                     // d
                     reg [42:0] r7;
                     reg [0:0] r8;
                     // d
                     reg [42:0] r9;
                     reg [18:0] r10;
                     reg [0:0] r11;
                     reg [15:0] r12;
                     reg [3:0] r13;
                     // d
                     reg [42:0] r14;
                     reg [0:0] r15;
                     reg [6:0] r16;
                     reg [4:0] r17;
                     reg [0:0] r18;
                     reg [3:0] r19;
                     reg [1:0] r20;
                     reg [0:0] r21;
                     reg [1:0] r22;
                     reg [1:0] r23;
                     // d
                     reg [42:0] r24;
                     // d
                     reg [42:0] r25;
                     // d
                     reg [42:0] r26;
                     // run
                     reg [0:0] r27;
                     // d
                     reg [42:0] r28;
                     // d
                     reg [42:0] r29;
                     // run
                     reg [0:0] r30;
                     reg [0:0] r31;
                     // d
                     reg [42:0] r32;
                     reg [6:0] r33;
                     reg [4:0] r34;
                     reg [0:0] r35;
                     reg [3:0] r36;
                     // d
                     reg [42:0] r37;
                     // d
                     reg [42:0] r38;
                     // d
                     reg [42:0] r39;
                     // d
                     reg [42:0] r40;
                     // run
                     reg [0:0] r41;
                     // d
                     reg [42:0] r42;
                     // run
                     reg [0:0] r43;
                     // write
                     reg [0:0] r44;
                     // d
                     reg [42:0] r45;
                     // run
                     reg [0:0] r46;
                     // write
                     reg [0:0] r47;
                     reg [15:0] r48;
                     reg [3:0] r49;
                     reg [15:0] r50;
                     reg [3:0] r51;
                     reg [3:0] r52;
                     // d
                     reg [42:0] r53;
                     reg [15:0] r54;
                     reg [3:0] r55;
                     reg [15:0] r56;
                     reg [3:0] r57;
                     reg [3:0] r58;
                     // d
                     reg [42:0] r59;
                     reg [15:0] r60;
                     reg [3:0] r61;
                     reg [15:0] r62;
                     reg [3:0] r63;
                     reg [3:0] r64;
                     // d
                     reg [42:0] r65;
                     // d
                     reg [42:0] r66;
                     reg [15:0] r67;
                     reg [3:0] r68;
                     // tmp
                     reg [15:0] r69;
                     reg [15:0] r70;
                     reg [3:0] r71;
                     // tmp
                     reg [15:0] r72;
                     reg [15:0] r73;
                     reg [3:0] r74;
                     // tmp
                     reg [15:0] r75;
                     reg [15:0] r76;
                     reg [3:0] r77;
                     // tmp
                     reg [15:0] r78;
                     reg [16:0] r79;
                     reg [15:0] r80;
                     reg [16:0] r81;
                     // d
                     reg [42:0] r82;
                     reg [18:0] r83;
                     reg [16:0] r84;
                     reg [6:0] r85;
                     reg [0:0] r86;
                     reg [17:0] r87;
                     reg [17:0] r88;
                     reg [60:0] r89;
                     reg [1:0] r90;
                     localparam l0 = 43'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 2'b11;
                     localparam l2 = 2'b01;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 1'b0;
                     localparam l8 = 1'b0;
                     localparam l9 = 2'b01;
                     localparam l10 = 2'b00;
                     localparam l11 = 1'b1;
                     localparam l12 = 1'b1;
                     localparam l13 = 1'b1;
                     localparam l14 = 1'b0;
                     localparam l15 = 1'b0;
                     localparam l16 = 1'b1;
                     localparam l17 = 16'bXXXXXXXXXXXXXXXX;
                     localparam l18 = 1'b1;
                     localparam l19 = 17'b00000000000000000;
                     localparam l20 = 18'b000000000000000000;
                     begin
                        r90 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[4:0];
                        r2 = l0;
                        r2[4:0] = r0;
                        r3 = r1[5:5];
                        r4 = r2;
                        r4[41:41] = r3;
                        r5 = r6[24:23];
                        r7 = r4;
                        r7[23:22] = r5;
                        r8 = r6[44:44];
                        r9 = r7;
                        r9[42:42] = r8;
                        r10 = r6[43:25];
                        r11 = r10[17:17];
                        r12 = r6[22:7];
                        r13 = r12[3:0];
                        r14 = r9;
                        r14[9:6] = r13;
                        r15 = r6[44:44];
                        r16 = r6[6:0];
                        r17 = r16[4:0];
                        r18 = r17[4:4];
                        r19 = r17[3:0];
                        r20 = r6[24:23];
                        r21 = r20 != l1;
                        r22 = r6[24:23];
                        r23 = r22 + l2;
                        r24 = r14;
                        r24[23:22] = r23;
                        r25 = r14;
                        r25[42:42] = l3;
                        r26 = r21 ? r24 : r25;
                        r27 = r21 ? l4 : l5;
                        r28 = r26;
                        r28[9:6] = r19;
                        case (r18)
                           1'b1 : r29 = r28;
                           default : r29 = r14;
                        endcase
                        case (r18)
                           1'b1 : r30 = r27;
                           default : r30 = l7;
                        endcase
                        r31 = ~r11;
                        r32 = r14;
                        r32[42:42] = l8;
                        r33 = r6[6:0];
                        r34 = r33[4:0];
                        r35 = r34[4:4];
                        r36 = r34[3:0];
                        r37 = r32;
                        r37[23:22] = l9;
                        r38 = r37;
                        r38[9:6] = r36;
                        r39 = r32;
                        r39[23:22] = l10;
                        case (r35)
                           1'b1 : r40 = r38;
                           default : r40 = r39;
                        endcase
                        case (r35)
                           1'b1 : r41 = l12;
                           default : r41 = l7;
                        endcase
                        r42 = r31 ? r40 : r14;
                        r43 = r31 ? r41 : l7;
                        r44 = r31 ? l13 : l14;
                        case (r15)
                           1'b0 : r45 = r29;
                           1'b1 : r45 = r42;
                        endcase
                        case (r15)
                           1'b0 : r46 = r30;
                           1'b1 : r46 = r43;
                        endcase
                        case (r15)
                           1'b0 : r47 = l14;
                           1'b1 : r47 = r44;
                        endcase
                        r48 = r6[22:7];
                        r49 = r48[3:0];
                        r50 = r6[22:7];
                        r51 = r50[7:4];
                        r52 = r46 ? r49 : r51;
                        r53 = r45;
                        r53[13:10] = r52;
                        r54 = r6[22:7];
                        r55 = r54[7:4];
                        r56 = r6[22:7];
                        r57 = r56[11:8];
                        r58 = r46 ? r55 : r57;
                        r59 = r53;
                        r59[17:14] = r58;
                        r60 = r6[22:7];
                        r61 = r60[11:8];
                        r62 = r6[22:7];
                        r63 = r62[15:12];
                        r64 = r46 ? r61 : r63;
                        r65 = r59;
                        r65[21:18] = r64;
                        r66 = r65;
                        r66[5:5] = r46;
                        r67 = r6[22:7];
                        r68 = r67[3:0];
                        r69 = l17;
                        r69[15:12] = r68;
                        r70 = r6[22:7];
                        r71 = r70[7:4];
                        r72 = r69;
                        r72[11:8] = r71;
                        r73 = r6[22:7];
                        r74 = r73[11:8];
                        r75 = r72;
                        r75[7:4] = r74;
                        r76 = r6[22:7];
                        r77 = r76[15:12];
                        r78 = r75;
                        r78[3:0] = r77;
                        r80 = r78[15:0];
                        r79 = {l18, r80};
                        r81 = r47 ? r79 : l19;
                        r82 = r66;
                        r82[40:24] = r81;
                        r83 = r6[43:25];
                        r84 = r83[16:0];
                        r85 = r6[6:0];
                        r86 = r85[5:5];
                        r87 = l20;
                        r87[16:0] = r84;
                        r88 = r87;
                        r88[17:17] = r86;
                        r89 = {r82, r88};
                        kernel_kernel = r89;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = Chunked::<b4, 2, 4>::default();
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
        let uut = Chunked::<b4, 2, 4>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_chunked");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "10bb8286533231f7a1206b744b50f1861d52a223236b86354cead0b7e578456e"
        ];
        let digest = vcd.dump_to_file(root.join("stream_chunked.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation_n_is_2() -> miette::Result<()> {
        test_operation_for_n::<1, 2>()?;
        Ok(())
    }

    #[test]
    fn test_operation_n_is_4() -> miette::Result<()> {
        test_operation_for_n::<2, 4>()?;
        Ok(())
    }

    fn test_operation_for_n<const M: usize, const N: usize>() -> miette::Result<()>
    where
        [b4; N]: Default,
        rhdl::bits::W<M>: BitWidth,
        rhdl::bits::W<N>: BitWidth,
    {
        let uut = Chunked::<b4, M, N>::default();
        let mut need_reset = true;
        let mut source_rng = XorShift128::default().map(|x| bits((x & 0xF) as u128));
        let dest_rng = source_rng.clone();
        let mut dest_rng = mk_array(dest_rng);
        let mut latched_input: Option<b4> = None;
        uut.run_fn(
            move |out| {
                if need_reset {
                    need_reset = false;
                    return Some(rhdl::core::sim::ResetOrData::Reset);
                }
                let mut input = super::In::<b4, N>::dont_care();
                // Downstream is likely to run
                let want_to_pause = rand::random::<u8>() > 200;
                input.ready = ready(!want_to_pause);
                // Decide if the producer will generate a new data item
                let willing_to_send = rand::random::<u8>() < 200;
                if out.ready.raw {
                    // The pipeline wants more data
                    if willing_to_send {
                        latched_input = source_rng.next();
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
