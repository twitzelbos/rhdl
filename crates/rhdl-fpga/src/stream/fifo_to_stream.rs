//! A FIFO-to-Stream buffer
//!
//!# Purpose
//! A FIFO-to-Stream buffer is a highly specialized two element FIFO backed with a pair
//! of registers instead of a BRAM.  The idea is to allow logic that has "push" semantics
//! like a FIFO interface, to connecto to stream, which has "pull" semantics.  Both
//! interfaces support backpressure (the FIFO via the `full` signal, and the stream via
//! the `ready` signal).
//!
//! The other way to conceptualize this is as a source and sink pair.  The supply side
//! pipeline is a data source - it produces data elements at it's own pace.  The demand
//! side pipeline is a data sink - it consumes data elements at it's own pace.  The push
//! pull buffer in general would be a FIFO, and indeed the Carloni papers show FIFOs as
//! the implementation of push-pull buffers.  However, in many cases, we only need minimal
//! buffering, and a pair of registers is sufficient.  
//!
//! Note that a normal synchronous FIFO as included in `rhdl` will not work here - if it has
//! only two slots in it, it cannot (by design) fill both slots.
//!
//! The design of the buffer uses a state to manage the fill level, and uses the extra
//! value of a fill level of 3 to indicate that the push-pull buffer is in an error condition
//! due to overflow of the input.  To make this buffer easy to use with Carloni
//! skid buffers, the output is presented as an `Option<T>`, and underflow is not possible,
//! as `None` is returned when the buffer is empty.
//!
//! Note that one use case for the [FIFOToStream] buffer is when we need to be able to
//! anticipate by a clock cycle that a pipeline is able to push data forward.  Here is
//! an example of the problem:
//!
#![doc = badascii!("
             +---------+               
ready  +-----+         +--------------+
                                       
                       +---+Some+-----+
data   +---------------+     T         
                                       
             +----+    +----+    +----+
clk     +----+    +----+    +----+     
                                       
             <--+t1+-->|<----+t2+----->
")]
//!
//! During the time `t1`, the downstream pipeline was available for us to push a new data item,
//! but our upstream process was not ready.  In time `t2`, the downstream pipeline is no longer
//! available, but the upstream process has produced a data item.  The upstream process must
//! stall and hold this output value until the downstream pipeline re-raises `ready` for a clock
//! cycle.
//!
//! With the [FIFOToStream] buffer, we have an addition invariant:
//!   - A FIFO that is not `full` on cycle `T` cannot be full on cycle `T+1` if we do not add data to it.
//!
//! This invariant means that the equivalent timing diagram with a [FIFOToStream] buffer
//! looks like this instead
//!
#![doc = badascii!("
       +-----+                         
full         +------------------------+
             :         :               
             :         +---+Some+-----+
data   +---------------+     T         
             :         :               
             +----+    +----+    +----+
clk     +----+    +----+    +----+     
                                       
             <--+t1+-->|<----+t2+----->
")]
//!
//! The important difference is that if the input stage is `!full`, as happens in interval `t1`, the
//! upstream pipeline is guaranteed to be able to run and produce a data item, even if it is in the
//! future.  Thus, if the upstream pipeline waits for the output to be `!full`, it can gaurantee that
//! one output item can be produced.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [FIFOToStream] buffer
//!
#![doc = badascii_formal!("
     ++FifoToStrm+--+     
 ?T  |              +?T   
+--->|data     data +---->
     |              |R<T>     
<----+full     ready|<---+
     |              |     
<----+error         |     
     +--------------+     
")]
//!
//!# Internals
//!
//! Effectively, the [FIFOToStream] buffer is simply a 2-element FIFO.  It is implemented with
//! a pair of registers and manual control logic, since the general FIFO logic does not handle
//! such small sizes well.
//!
//! Roughly the internal circuitry is equivalent to this:
//!
#![doc = badascii!(r"
 ?T  +----+FIFO+----+  ?T             
+--->|data      data+--------+--->    
     |              |        |is_some 
<----+full      next|<---+   +        
     |              |    +--+&        
     |              |        +  ready 
     +--------------+        +-------+
")]
//! The FIFO is advanced only if the output is `Some`, and if the `ready` signal is asserted.
//!
//! Note that there are no combinatorial paths between the inputs and
//! outputs, and a test is used to verify this property.
//!
//!# Example
//!
//! Here is an example of the interface.
//!
//!```
#![doc = include_str!("../../examples/fifo_to_stream.rs")]
//!```
//!
//! With an output.
//!
#![doc = include_str!("../../doc/fifo_to_rv.md")]
//!
use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::{
    core::{dff, option::is_some},
    stream::StreamIO,
};

#[derive(PartialEq, Digital, Copy, Default, Debug, Clone)]
#[doc(hidden)]
pub enum State {
    #[default]
    Empty,
    OneLoaded,
    TwoLoaded,
    Error,
}

#[derive(PartialEq, Debug, Clone, SynchronousDQ, Synchronous)]
#[rhdl(dq_no_prefix)]
/// The [FIFOToStream] Buffer core.
///
/// `T` is the type of the data elements flowing in the pipeline.
pub struct FIFOToStream<T: Digital> {
    /// The state of the buffer
    state: dff::DFF<State>,
    /// The 0 slot of the buffer,
    zero_slot: dff::DFF<T>,
    /// The 1 slot of the buffer,
    one_slot: dff::DFF<T>,
    /// Where to write next item - in this case
    /// we use false for zero and true for one
    write_slot: dff::DFF<bool>,
    /// Where to read next item
    read_slot: dff::DFF<bool>,
}

impl<T: Digital> Default for FIFOToStream<T> {
    fn default() -> Self {
        Self {
            state: dff::DFF::default(),
            zero_slot: dff::DFF::new(T::dont_care()),
            one_slot: dff::DFF::new(T::dont_care()),
            write_slot: dff::DFF::default(),
            read_slot: dff::DFF::default(),
        }
    }
}

/// Inputs to the [FIFOToStream] buffer
///
/// For inputs, the push pull buffer has a Option<T> input to combine the
/// write enable with the data signal, and provides a full signal back.
/// It is important that the full signal is not dependant on the consumer,
/// so that the pull-pull buffer isolates the producer from the consumer
/// and vice versa.
pub type In<T> = StreamIO<T, T>;

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [FIFOToStream] buffer
pub struct Out<T: Digital> {
    /// The consumers data
    pub data: Option<T>,
    /// The producers "Q is full" signal
    pub full: bool,
    /// An error flag to indicate that the core has
    /// overflowed.  This occurs if the producer attempts
    /// to write data when the FIFO is full.
    pub error: bool,
}

impl<T: Digital> SynchronousIO for FIFOToStream<T> {
    type I = StreamIO<T, T>;
    type O = Out<T>;
    type Kernel = kernel<T>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<T: Digital>(_cr: ClockReset, i: In<T>, q: Q<T>) -> (Out<T>, D<T>) {
    let mut d = D::<T>::dont_care();
    let will_write = is_some::<T>(i.data);
    let can_read = i.ready;
    // Update the state machine
    d.state = match q.state {
        State::Empty => {
            if will_write {
                State::OneLoaded
            } else {
                State::Empty
            }
        }
        State::OneLoaded => match (will_write, can_read.raw) {
            (false, false) => State::OneLoaded, // No change
            (true, false) => State::TwoLoaded,  // Producer wants to write
            (false, true) => State::Empty, // Consumer can read, and we have valid data, so we will be empty.
            (true, true) => State::OneLoaded, // Consumer can read, we have valid data, and producer wants to write.
        },
        State::TwoLoaded => {
            // Any write in this state is an error
            if will_write {
                State::Error
            } else if can_read.raw {
                State::OneLoaded
            } else {
                State::TwoLoaded
            }
        }
        State::Error => State::Error,
    };
    // If we will write on this cycle, then copy the
    // data into the appropriate slot and then switch
    // buffers.  The buffers are otherwise unchanged.
    d.zero_slot = q.zero_slot;
    d.one_slot = q.one_slot;
    if let Some(data) = i.data {
        if !q.write_slot {
            d.zero_slot = data;
        } else {
            d.one_slot = data;
        }
    }
    let next_item = can_read.raw && q.state != State::Empty && q.state != State::Error;
    // Toggle the read and write slots.
    d.write_slot = will_write ^ q.write_slot;
    d.read_slot = next_item ^ q.read_slot;
    // The output is set to void if we are empty, otherwise
    // the contents of the designated read slot
    let mut o = Out::<T>::dont_care();
    if q.state == State::Empty {
        o.data = None
    } else if !q.read_slot {
        o.data = Some(q.zero_slot);
    } else {
        o.data = Some(q.one_slot);
    };
    o.full = q.state == State::TwoLoaded;
    o.error = q.state == State::Error;
    (o, d)
}

#[cfg(test)]
mod tests {
    use crate::{rng::xorshift::XorShift128, stream::ready};

    use super::*;

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = FIFOToStream::<b16>::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// This widget's *producer* side is a FIFO write (gated by `full`)
    /// and its consumer side is Ready/Valid. Offers gapped on 4 and
    /// `ready` withheld on 3, coprime, so the buffer both fills toward
    /// `full` and drains rather than sitting at one occupancy.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4>)>> {
        (0..28u128)
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
        let uut = FIFOToStream::<b4>::default();
        let desc = uut.descriptor("fifo_to_stream".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "fifo_to_stream")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module fifo_to_stream(input wire [1:0] clock_reset, input wire [5:0] i, output wire [6:0] o);
               wire [18:0] od;
               wire [11:0] d;
               wire [11:0] q;
               assign o = od[6:0];
               fifo_to_stream_state c0(.clock_reset(clock_reset), .i(d[1:0]), .o(q[1:0]));
               fifo_to_stream_zero_slot c1(.clock_reset(clock_reset), .i(d[5:2]), .o(q[5:2]));
               fifo_to_stream_one_slot c2(.clock_reset(clock_reset), .i(d[9:6]), .o(q[9:6]));
               fifo_to_stream_write_slot c3(.clock_reset(clock_reset), .i(d[10:10]), .o(q[10:10]));
               fifo_to_stream_read_slot c4(.clock_reset(clock_reset), .i(d[11:11]), .o(q[11:11]));
               assign d = od[18:7];
               assign od = kernel_kernel(clock_reset, i, q);
               function [18:0] kernel_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [11:0] arg_2);
                     reg [4:0] r0;
                     reg [5:0] r1;
                     reg [0:0] r2;
                     reg [0:0] r3;
                     reg [0:0] r4;
                     reg [1:0] r5;
                     reg [11:0] r6;
                     reg [1:0] r7;
                     reg [1:0] r8;
                     reg [1:0] r9;
                     reg [1:0] r10;
                     reg [1:0] r11;
                     reg [1:0] r12;
                     // d
                     reg [11:0] r13;
                     reg [3:0] r14;
                     // d
                     reg [11:0] r15;
                     reg [3:0] r16;
                     // d
                     reg [11:0] r17;
                     reg [4:0] r18;
                     reg [0:0] r19;
                     reg [3:0] r20;
                     reg [0:0] r21;
                     reg [0:0] r22;
                     // d
                     reg [11:0] r23;
                     // d
                     reg [11:0] r24;
                     // d
                     reg [11:0] r25;
                     // d
                     reg [11:0] r26;
                     reg [1:0] r27;
                     reg [0:0] r28;
                     reg [0:0] r29;
                     reg [1:0] r30;
                     reg [0:0] r31;
                     reg [0:0] r32;
                     reg [0:0] r33;
                     reg [0:0] r34;
                     // d
                     reg [11:0] r35;
                     reg [0:0] r36;
                     reg [0:0] r37;
                     // d
                     reg [11:0] r38;
                     reg [1:0] r39;
                     reg [0:0] r40;
                     reg [0:0] r41;
                     reg [0:0] r42;
                     reg [3:0] r43;
                     reg [4:0] r44;
                     reg [3:0] r45;
                     // o
                     reg [6:0] r46;
                     reg [3:0] r47;
                     reg [4:0] r48;
                     reg [3:0] r49;
                     // o
                     reg [6:0] r50;
                     // o
                     reg [6:0] r51;
                     // o
                     reg [6:0] r52;
                     reg [1:0] r53;
                     reg [0:0] r54;
                     // o
                     reg [6:0] r55;
                     reg [1:0] r56;
                     reg [0:0] r57;
                     // o
                     reg [6:0] r58;
                     reg [18:0] r59;
                     reg [1:0] r60;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b0;
                     localparam l4 = 2'b01;
                     localparam l5 = 2'b00;
                     localparam l6 = 2'b00;
                     localparam l7 = 2'b01;
                     localparam l8 = 2'b01;
                     localparam l9 = 2'b10;
                     localparam l10 = 2'b10;
                     localparam l11 = 2'b00;
                     localparam l12 = 2'b11;
                     localparam l13 = 2'b01;
                     localparam l14 = 2'b01;
                     localparam l15 = 2'b10;
                     localparam l16 = 2'b11;
                     localparam l17 = 2'b00;
                     localparam l18 = 2'b01;
                     localparam l19 = 2'b10;
                     localparam l20 = 2'b11;
                     localparam l21 = 2'b11;
                     localparam l22 = 12'bXXXXXXXXXXXX;
                     localparam l23 = 1'b1;
                     localparam l24 = 2'b11;
                     localparam l25 = 2'b00;
                     localparam l26 = 1'b1;
                     localparam l27 = 7'bXXXXXXX;
                     localparam l28 = 1'b1;
                     localparam l29 = 7'bXX00000;
                     localparam l30 = 2'b10;
                     localparam l31 = 2'b11;
                     begin
                        r60 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[4:0];
                        r2 = r0[4:4];
                        case (r2)
                           1'b1 : r3 = l1;
                           1'b0 : r3 = l3;
                        endcase
                        r4 = r1[5:5];
                        r5 = r6[1:0];
                        r7 = r3 ? l4 : l5;
                        r8 = {r4, r3};
                        case (r8)
                           2'b00 : r9 = l7;
                           2'b01 : r9 = l9;
                           2'b10 : r9 = l11;
                           2'b11 : r9 = l13;
                        endcase
                        r10 = r4 ? l14 : l15;
                        r11 = r3 ? l16 : r10;
                        case (r5)
                           2'b00 : r12 = r7;
                           2'b01 : r12 = r9;
                           2'b10 : r12 = r11;
                           2'b11 : r12 = l21;
                        endcase
                        r13 = l22;
                        r13[1:0] = r12;
                        r14 = r6[5:2];
                        r15 = r13;
                        r15[5:2] = r14;
                        r16 = r6[9:6];
                        r17 = r15;
                        r17[9:6] = r16;
                        r18 = r1[4:0];
                        r19 = r18[4:4];
                        r20 = r18[3:0];
                        r21 = r6[10:10];
                        r22 = ~r21;
                        r23 = r17;
                        r23[5:2] = r20;
                        r24 = r17;
                        r24[9:6] = r20;
                        r25 = r22 ? r23 : r24;
                        case (r19)
                           1'b1 : r26 = r25;
                           default : r26 = r17;
                        endcase
                        r27 = r6[1:0];
                        r28 = |r27;
                        r29 = r4 & r28;
                        r30 = r6[1:0];
                        r31 = r30 != l24;
                        r32 = r29 & r31;
                        r33 = r6[10:10];
                        r34 = r3 ^ r33;
                        r35 = r26;
                        r35[10:10] = r34;
                        r36 = r6[11:11];
                        r37 = r32 ^ r36;
                        r38 = r35;
                        r38[11:11] = r37;
                        r39 = r6[1:0];
                        r40 = r39 == l25;
                        r41 = r6[11:11];
                        r42 = ~r41;
                        r43 = r6[5:2];
                        r45 = r43[3:0];
                        r44 = {l26, r45};
                        r46 = l27;
                        r46[4:0] = r44;
                        r47 = r6[9:6];
                        r49 = r47[3:0];
                        r48 = {l28, r49};
                        r50 = l27;
                        r50[4:0] = r48;
                        r51 = r42 ? r46 : r50;
                        r52 = r40 ? l29 : r51;
                        r53 = r6[1:0];
                        r54 = r53 == l30;
                        r55 = r52;
                        r55[5:5] = r54;
                        r56 = r6[1:0];
                        r57 = r56 == l31;
                        r58 = r55;
                        r58[6:6] = r57;
                        r59 = {r38, r58};
                        kernel_kernel = r59;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut = FIFOToStream::<b4>::default();
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
        let uut = FIFOToStream::<b4>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("fifo_to_stream");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "3d0207215877bccb1365e863cbef6954207b9d9baaba0647cfe83b6041bed793"
        ];
        let digest = vcd.dump_to_file(root.join("fifo_to_stream.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> miette::Result<()> {
        // The buffer will manage items of 4 bits
        let uut = FIFOToStream::<b4>::default();
        // The test harness will include a consumer that
        // randomly pauses the upstream producer.
        let mut need_reset = true;
        let mut checked = 0usize;
        let mut source_rng = XorShift128::default().map(|x| bits((x & 0xF) as u128));
        let mut dest_rng = source_rng.clone();
        uut.run_fn(
            |out| {
                if need_reset {
                    need_reset = false;
                    return Some(rhdl::core::sim::ResetOrData::Reset);
                }
                let mut input = super::In::<b4>::dont_care();
                let want_to_pause = rand::random::<u8>() > 200;
                input.ready = ready(!want_to_pause);
                // Decide if the producer will generate a data item
                let want_to_send = rand::random::<u8>() < 200;
                input.data = None;
                if !out.full && want_to_send {
                    input.data = source_rng.next();
                }
                if out.data.is_some() && input.ready.raw {
                    assert_eq!(out.data, dest_rng.next());
                    checked += 1;
                }
                Some(rhdl::core::sim::ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 100_000)
        .for_each(drop);
        // The comparison above is the only assertion in this file, and it
        // sits inside a `Some`-guard: a buffer that delivered nothing
        // would run it zero times and pass. Count the comparisons that
        // actually happened.
        assert!(
            checked > 10,
            "expected the buffer to deliver items to compare; got {checked}"
        );
        Ok(())
    }
}
