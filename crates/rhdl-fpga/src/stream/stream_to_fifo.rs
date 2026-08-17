//! A Stream-to-FIFO buffer
//!
//!# Purpose
//! A Stream-to-FIFO buffer is a highly specialized two element
//! FIFO backed with a pair of registers instead of a BRAM.  
//! Note that a FIFO cannot be interfaced to a stream by
//! simply setting `ready = !full`.  This is because the FIFO
//! interface contract says that setting `data = Some(_)` when
//! `full = true` leads to an overflow. The particular use of this
//! [StreamToFIFO] buffer is to minimize the number of resources
//! needed.  As it only requires a couple of registers, it is generally
//! far less resource intensive than a full FIFO.
//!
//!# Schematic Symbol
//!
//! Here is the schematic symbol for the [StreamToFIFO] buffer
//!
#![doc = badascii_formal!("
     ++Stm2FIFO+--+     
 ?T  |            | ?T  
+--->|data    data+---->
R<T> |            |     
<----+ready   next|<---+
     |            |     
     |       error+---->
     +------------+     
")]
//!
//!# Internals
//!
//! Effectively, the [StreamToFIFO] buffer is simply a 2-element FIFO.
//! It is implemented with a pair of registers and manual control logic,
//! since the general FIFO logic doesn't work with such small sizes.
//!
//! Roughly the internal circuitry is equivalent to this:
//!
#![doc = badascii!("
     ?T       +----+FIFO+----+ ?T  
+------------>|data      data+---->
  Ready<T>    |              |     
<-----+! <----+full      next|<---+
              +--------------+     
")]
//! Unlike the general FIFOs, it will not overflow, as it assumes that the
//! data signal will be held until the `Ready` signal is provided.  This is
//! different from normal FIFOs, which will overflow under those conditions.
//!  The FIFO will signal that it is `ready` as long as it is not `full`.
//! The consumer can use the `next` signal to accept the current `data`
//! element.
//!
//! Note that there are no combinatorial paths between the inputs and
//! outputs, and a test is used to verify this property.
//!
//!# Example
//!
//! Here is an example of the buffer in action.
//!
//!```
#![doc = include_str!("../../examples/stream_to_fifo.rs")]
//!```
//!
//! With the output.
#![doc = include_str!("../../doc/rv_to_fifo.md")]

use badascii_doc::{badascii, badascii_formal};
use rhdl::prelude::*;

use crate::{
    core::{dff, option::is_some},
    stream::ready,
};

use super::Ready;

/// A READY/VALID-to-FIFO converter is a highly specialized two element
/// FIFO backed with a pair of registers instead of a BRAM.  
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
/// The [StreamToFIFO] Buffer Core
///
/// `T` is the type of the data elements flowing in the pipeline.
pub struct StreamToFIFO<T: Digital> {
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

impl<T: Digital> Default for StreamToFIFO<T> {
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

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [StreamToFIFO] buffer
///
/// For inputs, we accept an `Option<T>` input from the ready/valid bus
/// and a next signal to acknowledge that data had been consumed.
/// The output is an `Option<T>` and a ready signal to provide backpressure.
/// This buffer cannot overflow, since it consumes incoming data only when
/// ready.  However, it can underflow if the receiver signals a next
/// when there is no data available.
pub struct In<T: Digital> {
    /// The data from the bus
    pub data: Option<T>,
    /// The next signal from the consumer
    pub next: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [StreamToFIFO] buffer
pub struct Out<T: Digital> {
    /// The data to the consumer
    pub data: Option<T>,
    /// The ready signal to the producer
    pub ready: Ready<T>,
    /// An error flag to indicate that the core has
    /// underflowed.
    pub error: bool,
}

impl<T: Digital> SynchronousIO for StreamToFIFO<T> {
    type I = In<T>;
    type O = Out<T>;
    type Kernel = kernel<T>;
}

#[kernel]
#[doc(hidden)]
pub fn kernel<T: Digital>(_cr: ClockReset, i: In<T>, q: Q<T>) -> (Out<T>, D<T>) {
    let mut d = D::<T>::dont_care();
    let will_read = i.next;
    let can_write = is_some::<T>(i.data);
    // Update the state machine
    d.state = match q.state {
        State::Empty => {
            if can_write {
                State::OneLoaded
            } else if will_read {
                State::Error
            } else {
                State::Empty
            }
        }
        State::OneLoaded => match (can_write, will_read) {
            (false, false) => State::OneLoaded, // No change
            (true, false) => State::TwoLoaded, // Producer wants to write, consumer does not want to read
            (false, true) => State::Empty, // Consumer can read, and we have valid data, so we will be empty.
            (true, true) => State::OneLoaded, // Consumer can read, we have valid data, and producer wants to write.
        },
        State::TwoLoaded => {
            if will_read {
                State::OneLoaded
            } else {
                State::TwoLoaded
            }
        }
        State::Error => State::Error,
    };
    let write_is_allowed = q.state != State::TwoLoaded && q.state != State::Error;
    // Decide if we will write on this clock cycle
    let will_write = can_write && write_is_allowed;
    d.zero_slot = q.zero_slot;
    d.one_slot = q.one_slot;
    if let Some(data) = i.data {
        if will_write {
            if q.write_slot {
                d.one_slot = data;
            } else {
                d.zero_slot = data;
            }
        }
    }
    d.write_slot = will_write ^ q.write_slot;
    d.read_slot = will_read ^ q.read_slot;
    let mut o = Out::<T>::dont_care();
    if q.state == State::Empty {
        o.data = None;
    } else if !q.read_slot {
        o.data = Some(q.zero_slot);
    } else {
        o.data = Some(q.one_slot);
    }
    o.ready = ready::<T>(write_is_allowed);
    o.error = q.state == State::Error;
    (o, d)
}

#[cfg(test)]
mod tests {
    use rhdl::prelude::*;

    use crate::rng::xorshift::XorShift128;

    use super::{In, StreamToFIFO};

    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = StreamToFIFO::<b16>::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Open-loop stimulus for Tiers 3-5.
    ///
    /// **The stall shape differs here.** This widget's downstream
    /// consumes with `next` — a pop request — not `ready`, so
    /// "withhold ready when idle" has no analogue: asserting `next` on
    /// an empty buffer is an underflow, which the widget already flags
    /// on its `error` output. The stalling done here is therefore on
    /// the *pop* side: `next` is asserted only 2 cycles in 3, so the
    /// buffer fills rather than draining as fast as it is written, and
    /// `ready` is exercised going low.
    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b4>)>> {
        (0..28u128)
            .map(|k| In::<b4> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(b4(k % 16))
                },
                next: !k.is_multiple_of(3),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = StreamToFIFO::<b4>::default();
        let desc = uut.descriptor("stream_to_fifo".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "stream_to_fifo")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module stream_to_fifo(input wire [1:0] clock_reset, input wire [5:0] i, output wire [6:0] o);
               wire [18:0] od;
               wire [11:0] d;
               wire [11:0] q;
               assign o = od[6:0];
               stream_to_fifo_state c0(.clock_reset(clock_reset), .i(d[1:0]), .o(q[1:0]));
               stream_to_fifo_zero_slot c1(.clock_reset(clock_reset), .i(d[5:2]), .o(q[5:2]));
               stream_to_fifo_one_slot c2(.clock_reset(clock_reset), .i(d[9:6]), .o(q[9:6]));
               stream_to_fifo_write_slot c3(.clock_reset(clock_reset), .i(d[10:10]), .o(q[10:10]));
               stream_to_fifo_read_slot c4(.clock_reset(clock_reset), .i(d[11:11]), .o(q[11:11]));
               assign d = od[18:7];
               assign od = kernel_kernel(clock_reset, i, q);
               function [18:0] kernel_kernel(input reg [1:0] arg_0, input reg [5:0] arg_1, input reg [11:0] arg_2);
                     reg [0:0] r0;
                     reg [5:0] r1;
                     reg [4:0] r2;
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
                     reg [1:0] r14;
                     reg [0:0] r15;
                     reg [1:0] r16;
                     reg [0:0] r17;
                     reg [0:0] r18;
                     reg [0:0] r19;
                     reg [3:0] r20;
                     // d
                     reg [11:0] r21;
                     reg [3:0] r22;
                     // d
                     reg [11:0] r23;
                     reg [4:0] r24;
                     reg [0:0] r25;
                     reg [3:0] r26;
                     reg [0:0] r27;
                     // d
                     reg [11:0] r28;
                     // d
                     reg [11:0] r29;
                     // d
                     reg [11:0] r30;
                     // d
                     reg [11:0] r31;
                     // d
                     reg [11:0] r32;
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
                     reg [0:0] r53;
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
                     localparam l4 = 2'b11;
                     localparam l5 = 2'b00;
                     localparam l6 = 2'b01;
                     localparam l7 = 2'b00;
                     localparam l8 = 2'b01;
                     localparam l9 = 2'b01;
                     localparam l10 = 2'b10;
                     localparam l11 = 2'b10;
                     localparam l12 = 2'b00;
                     localparam l13 = 2'b11;
                     localparam l14 = 2'b01;
                     localparam l15 = 2'b01;
                     localparam l16 = 2'b10;
                     localparam l17 = 2'b00;
                     localparam l18 = 2'b01;
                     localparam l19 = 2'b10;
                     localparam l20 = 2'b11;
                     localparam l21 = 2'b11;
                     localparam l22 = 12'bXXXXXXXXXXXX;
                     localparam l23 = 2'b10;
                     localparam l24 = 2'b11;
                     localparam l25 = 1'b1;
                     localparam l26 = 2'b00;
                     localparam l27 = 1'b1;
                     localparam l28 = 7'bXXXXXXX;
                     localparam l29 = 1'b1;
                     localparam l30 = 7'bXX00000;
                     localparam l31 = 1'b0;
                     localparam l32 = 2'b11;
                     begin
                        r60 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[5:5];
                        r2 = r1[4:0];
                        r3 = r2[4:4];
                        case (r3)
                           1'b1 : r4 = l1;
                           1'b0 : r4 = l3;
                        endcase
                        r5 = r6[1:0];
                        r7 = r0 ? l4 : l5;
                        r8 = r4 ? l6 : r7;
                        r9 = {r0, r4};
                        case (r9)
                           2'b00 : r10 = l8;
                           2'b01 : r10 = l10;
                           2'b10 : r10 = l12;
                           2'b11 : r10 = l14;
                        endcase
                        r11 = r0 ? l15 : l16;
                        case (r5)
                           2'b00 : r12 = r8;
                           2'b01 : r12 = r10;
                           2'b10 : r12 = r11;
                           2'b11 : r12 = l21;
                        endcase
                        r13 = l22;
                        r13[1:0] = r12;
                        r14 = r6[1:0];
                        r15 = r14 != l23;
                        r16 = r6[1:0];
                        r17 = r16 != l24;
                        r18 = r15 & r17;
                        r19 = r4 & r18;
                        r20 = r6[5:2];
                        r21 = r13;
                        r21[5:2] = r20;
                        r22 = r6[9:6];
                        r23 = r21;
                        r23[9:6] = r22;
                        r24 = r1[4:0];
                        r25 = r24[4:4];
                        r26 = r24[3:0];
                        r27 = r6[10:10];
                        r28 = r23;
                        r28[9:6] = r26;
                        r29 = r23;
                        r29[5:2] = r26;
                        r30 = r27 ? r28 : r29;
                        r31 = r19 ? r30 : r23;
                        case (r25)
                           1'b1 : r32 = r31;
                           default : r32 = r23;
                        endcase
                        r33 = r6[10:10];
                        r34 = r19 ^ r33;
                        r35 = r32;
                        r35[10:10] = r34;
                        r36 = r6[11:11];
                        r37 = r0 ^ r36;
                        r38 = r35;
                        r38[11:11] = r37;
                        r39 = r6[1:0];
                        r40 = r39 == l26;
                        r41 = r6[11:11];
                        r42 = ~r41;
                        r43 = r6[5:2];
                        r45 = r43[3:0];
                        r44 = {l27, r45};
                        r46 = l28;
                        r46[4:0] = r44;
                        r47 = r6[9:6];
                        r49 = r47[3:0];
                        r48 = {l29, r49};
                        r50 = l28;
                        r50[4:0] = r48;
                        r51 = r42 ? r46 : r50;
                        r52 = r40 ? l30 : r51;
                        r53 = l31;
                        r54 = r53;
                        r54[0:0] = r18;
                        r55 = r52;
                        r55[5:5] = r54;
                        r56 = r6[1:0];
                        r57 = r56 == l32;
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
        let uut = StreamToFIFO::<b4>::default();
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
        let uut = StreamToFIFO::<b4>::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("stream_to_fifo");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "0e2c7e9586efe6bbab68396d58eaf8688ce1d8323027db96ba273b05070a6205"
        ];
        let digest = vcd.dump_to_file(root.join("stream_to_fifo.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_operation() -> miette::Result<()> {
        // The buffer will manage items of 4 bits
        let uut = StreamToFIFO::<b4>::default();
        // The test harness will include a consumer that
        // randomly pauses the upstream producer.
        let mut need_reset = true;
        let mut source_rng = XorShift128::default().map(|x| bits((x & 0xF) as u128));
        let mut dest_rng = source_rng.clone();
        let mut source_datum = source_rng.next();
        uut.run_fn(
            |out| {
                if need_reset {
                    need_reset = false;
                    return Some(rhdl::core::sim::ResetOrData::Reset);
                }
                let mut input = super::In::<b4>::dont_care();
                let may_accept = rand::random::<u8>() > 150;
                let will_accept = may_accept & out.data.is_some();
                input.next = false;
                if will_accept {
                    assert_eq!(out.data, dest_rng.next());
                    input.next = true;
                }
                let will_offer = rand::random::<u8>() > 150;
                if will_offer {
                    input.data = source_datum;
                } else {
                    input.data = None;
                }
                let will_advance = will_offer & out.ready.raw;
                if will_advance {
                    source_datum = source_rng.next();
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
