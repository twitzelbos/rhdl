//! Turn one RDMA WRITE *message* into the sequence of *packets* that
//! carry it.
//!
//! This is the piece that makes the difference between generating a
//! packet and sending a message. An application asks to write `length`
//! bytes at a remote virtual address; the path MTU decides how many
//! packets that takes, and the IBTA decides what each one is called:
//!
//! ```text
//!   length <= PMTU            ->  ONLY
//!   length >  PMTU            ->  FIRST, MIDDLE*, LAST
//! ```
//!
//! Only FIRST and ONLY carry an [`Reth`]; the responder tracks the
//! running address for the rest. The packet sequence number increments
//! by one per packet, and the final packet's `padcount` covers the
//! ragged tail.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---+WriteSegmenter+------+
     |                         |
bool |                         |
+--->| start                   |
?Msg |                         | ?Plan
+--->| msg                plan +--->
?Ctx |                         |
+--->| ctx                busy +---> bool
B<24>|                         |
+--->| psn_start               |
bool |                         |
+--->| ready                   |
     +-------------------------+
")]
//!
//!# Internals
//!
//! One register holding one bundled state struct, per CLAUDE.md §3.1 —
//! this is a single state machine with several internal registers
//! rather than a composition of independent sub-circuits, so the
//! registers are bundled into one `Digital` struct instead of becoming
//! sibling `DFF` fields.
//!
//! There is deliberately **no state enum and no `FsmWidget` derive**.
//! The only sequential state is "a message is in progress", and a
//! two-state diagram would convey nothing that the `busy` output does
//! not. The interesting logic — which opcode this packet is — is a
//! function of `first` and `last`, not of a state register, so an FSM
//! diagram would actively mislead by suggesting otherwise.
#![doc = badascii_doc::badascii!(r"
   start, msg, psn_start
        |
        v
   +----+latch+----+     +-+DFF+--+
   | remaining     |---->|d      q|--+--> plan (combinational
   | psn, base_va  |     +--------+  |      from the register)
   | total_len     |                 |
   | first, active |<----------------+
   +---------------+   advance when ready
")]
//!
//!# Backpressure
//!
//! `plan` is `Some` on every cycle a packet is pending, and the state
//! advances only on a cycle where `ready` is also asserted. So a sink
//! that is not ready sees the *same* plan held stable rather than a
//! dropped one, which is the convention the rest of the library's
//! streaming widgets follow.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/segmenter.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/segmenter.md")]

use super::context::{MessageRequest, QpContext, pmtu_bytes};
use crate::headers::{Reth, WriteOpcode, carries_reth};
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Everything needed to emit one packet of a message.
///
/// The segmenter decides all of this; a downstream header generator
/// turns it into bytes without making any further decisions.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct PacketPlan {
    /// Which of the WRITE family this packet is.
    pub opcode: WriteOpcode,
    /// This packet's sequence number.
    pub psn: Bits<24>,
    /// How many payload bytes this packet carries, before padding.
    pub payload_bytes: Bits<13>,
    /// Pad bytes the sender appends to reach a 4-byte boundary, 0–3.
    /// Non-zero only on the final packet of a message.
    pub padcount: Bits<2>,
    /// True when this packet carries `reth`. Derived from `opcode`, but
    /// materialised so a downstream byte generator does not have to
    /// re-derive the header length.
    pub carries_reth: bool,
    /// The RDMA Extended Transport Header. Meaningful only when
    /// `carries_reth`; holds the message's *base* address and *total*
    /// length, not this packet's.
    pub reth: Reth,
}

#[kernel]
/// Pad bytes needed to bring `len` up to a 4-byte boundary.
///
/// `(4 - len % 4) % 4`, which is `(4 - (len & 3)) & 3`. The outer mask
/// is what turns the already-aligned case into 0 rather than 4.
pub fn pad_for(len: Bits<13>) -> Bits<2> {
    let low = len & bits(3);
    ((bits::<3>(4) - low.resize::<3>()) & bits(3)).resize::<2>()
}

#[kernel]
/// Select the WRITE opcode from a packet's position in its message.
///
/// The whole of the naming rule, in one place: a message that both
/// starts and ends in this packet is ONLY, and the three other
/// combinations are FIRST, LAST and MIDDLE.
pub fn write_opcode_for(first: bool, last: bool) -> WriteOpcode {
    if first && last {
        WriteOpcode::Only
    } else if first {
        WriteOpcode::First
    } else if last {
        WriteOpcode::Last
    } else {
        WriteOpcode::Middle
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
#[doc(hidden)]
/// Bundled internal state, per CLAUDE.md §3.1.
pub struct SegState {
    /// A message is in progress.
    pub active: bool,
    /// The next packet emitted will be the message's first.
    pub first: bool,
    /// Payload bytes still to send.
    pub remaining: Bits<32>,
    /// Sequence number of the next packet.
    pub psn: Bits<24>,
    /// The message's base address, latched for the RETH.
    pub base_va: Bits<64>,
    /// The message's total length, latched for the RETH's `dma_len`.
    pub total_len: Bits<32>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// Splits an RDMA WRITE message into path-MTU-sized packets.
pub struct WriteSegmenter {
    state: dff::DFF<SegState>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`WriteSegmenter`].
pub struct In {
    /// Begin a new message this cycle, latching `msg` and `psn_start`.
    ///
    /// Asserting this while a message is in progress **restarts**,
    /// abandoning the remainder. The caller is expected to wait for
    /// `busy` to fall; this is not policed, because policing it would
    /// mean a second error path for a condition a correct caller cannot
    /// produce.
    pub start: bool,
    /// The message to send. Sampled only when `start`.
    pub msg: MessageRequest,
    /// Connection parameters. Read continuously, since the path MTU and
    /// the R_Key are needed on every packet.
    pub ctx: QpContext,
    /// The sequence number for the message's first packet. Sampled only
    /// when `start`.
    pub psn_start: Bits<24>,
    /// The downstream consumer can accept a packet this cycle.
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of the [`WriteSegmenter`].
pub struct Out {
    /// The packet to send, or `None` when no message is in progress.
    /// Held stable until `ready`.
    pub plan: Option<PacketPlan>,
    /// A message is in progress.
    pub busy: bool,
}

impl SynchronousIO for WriteSegmenter {
    type I = In;
    type O = Out;
    type Kernel = write_segmenter;
}

#[kernel]
/// Kernel for [`WriteSegmenter`].
pub fn write_segmenter(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();
    let mut st = q.state;

    let pmtu = pmtu_bytes(i.ctx.pmtu).resize::<32>();

    // A zero-length message satisfies this too, and is a legal
    // single-packet WRITE that moves no data.
    let is_last = st.remaining <= pmtu;
    let this_len = if is_last { st.remaining } else { pmtu };
    let opcode = write_opcode_for(st.first, is_last);

    let mut reth = Reth::dont_care();
    reth.va = st.base_va;
    reth.rkey = i.ctx.rkey;
    reth.dma_len = st.total_len;

    let mut plan = PacketPlan::dont_care();
    plan.opcode = opcode;
    plan.psn = st.psn;
    plan.payload_bytes = this_len.resize::<13>();
    plan.padcount = pad_for(this_len.resize::<13>());
    plan.carries_reth = carries_reth(opcode);
    plan.reth = reth;

    o.plan = if st.active { Some(plan) } else { None };
    o.busy = st.active;

    // Advance only on an accepted packet, so an unready sink sees the
    // same plan held rather than a dropped one.
    if st.active && i.ready {
        st.first = false;
        st.psn += 1;
        st.remaining -= this_len;
        st.active = !is_last;
    }

    // `start` overrides the advance above, which is what makes
    // restarting mid-message well-defined rather than merely undefined.
    if i.start {
        st.active = true;
        st.first = true;
        st.remaining = i.msg.length;
        st.psn = i.psn_start;
        st.base_va = i.msg.va;
        st.total_len = i.msg.length;
    }

    d.state = st;

    if cr.reset.any() {
        d.state = SegState::dont_care();
        d.state.active = false;
        d.state.first = false;
        d.state.remaining = bits(0);
        d.state.psn = bits(0);
        d.state.base_va = bits(0);
        d.state.total_len = bits(0);
        o.plan = None;
        o.busy = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Transport;
    use crate::qp::context::Pmtu;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use std::path::PathBuf;

    fn ctx(pmtu: Pmtu) -> QpContext {
        QpContext {
            dqpn: bits(0x000111),
            transport: Transport::Uc,
            pmtu,
            rkey: bits(0x0000_A5A5),
            pkey: bits(0xFFFF),
        }
    }

    const VA: u128 = 0x0000_7F00_1234_0000;

    /// Run a message through the widget and collect the accepted plans.
    ///
    /// `ready` is held true, so one packet is accepted per cycle.
    fn plans(length: u128, pmtu: Pmtu, psn_start: u128) -> Vec<PacketPlan> {
        let idle = In {
            start: false,
            msg: MessageRequest {
                va: bits(VA),
                length: bits(length),
            },
            ctx: ctx(pmtu),
            psn_start: bits(psn_start),
            ready: true,
        };
        let mut stream = vec![In {
            start: true,
            ..idle
        }];
        // Enough cycles for the longest message under test, plus slack.
        for _ in 0..40 {
            stream.push(idle);
        }
        let uut = WriteSegmenter::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(input)
            .synchronous_sample()
            .filter_map(|s| s.output.plan)
            .collect()
    }

    // ---------------- Tier 1 ----------------

    #[test]
    fn pad_for_rounds_up_to_four() {
        assert_eq!(pad_for(bits(0)), bits::<2>(0));
        assert_eq!(pad_for(bits(1)), bits::<2>(3));
        assert_eq!(pad_for(bits(2)), bits::<2>(2));
        assert_eq!(pad_for(bits(3)), bits::<2>(1));
        assert_eq!(pad_for(bits(4)), bits::<2>(0));
        assert_eq!(pad_for(bits(13)), bits::<2>(3));
        assert_eq!(pad_for(bits(1024)), bits::<2>(0));
        // Exhaustive over the whole payload-length range: the value is
        // always what brings the length to a multiple of four.
        for n in 0u128..8192 {
            let p = pad_for(bits(n)).raw();
            assert_eq!((n + p) % 4, 0, "length {n} + pad {p} is not aligned");
            assert!(p < 4);
        }
    }

    #[test]
    fn opcode_selection_covers_all_four_positions() {
        assert_eq!(write_opcode_for(true, true), WriteOpcode::Only);
        assert_eq!(write_opcode_for(true, false), WriteOpcode::First);
        assert_eq!(write_opcode_for(false, true), WriteOpcode::Last);
        assert_eq!(write_opcode_for(false, false), WriteOpcode::Middle);
    }

    /// Only FIRST and ONLY carry a RETH. This ties the opcode selection
    /// to the header length, which is where getting it wrong puts the
    /// payload inside the RETH.
    #[test]
    fn reth_is_carried_exactly_on_first_and_only() {
        for (first, last) in [(true, true), (true, false), (false, true), (false, false)] {
            let op = write_opcode_for(first, last);
            assert_eq!(
                carries_reth(op),
                first,
                "{op:?} should carry a RETH iff it is the message's first packet"
            );
        }
    }

    #[test]
    fn pmtu_bytes_are_the_five_legal_values() {
        assert_eq!(pmtu_bytes(Pmtu::B256), bits::<13>(256));
        assert_eq!(pmtu_bytes(Pmtu::B512), bits::<13>(512));
        assert_eq!(pmtu_bytes(Pmtu::B1024), bits::<13>(1024));
        assert_eq!(pmtu_bytes(Pmtu::B2048), bits::<13>(2048));
        assert_eq!(pmtu_bytes(Pmtu::B4096), bits::<13>(4096));
    }

    #[test]
    fn reset_clears_the_state() {
        let cr = clock_reset(clock(false), reset(true));
        let i = In {
            start: true,
            msg: MessageRequest {
                va: bits(VA),
                length: bits(600),
            },
            ctx: ctx(Pmtu::B256),
            psn_start: bits(9),
            ready: true,
        };
        let q = Q {
            state: SegState {
                active: true,
                first: false,
                remaining: bits(123),
                psn: bits(7),
                base_va: bits(VA),
                total_len: bits(600),
            },
        };
        let (o, d) = write_segmenter(cr, i, q);
        assert!(!d.state.active);
        assert_eq!(d.state.remaining, bits::<32>(0));
        assert_eq!(o.plan, None);
        assert!(!o.busy);
    }

    /// An unready sink must see the same plan held, not a dropped one.
    #[test]
    fn an_unready_sink_holds_the_plan() {
        let cr = ClockReset::dont_care();
        let q = Q {
            state: SegState {
                active: true,
                first: true,
                remaining: bits(600),
                psn: bits(0x10),
                base_va: bits(VA),
                total_len: bits(600),
            },
        };
        let i = In {
            start: false,
            msg: MessageRequest::dont_care(),
            ctx: ctx(Pmtu::B256),
            psn_start: bits(0),
            ready: false,
        };
        let (o, d) = write_segmenter(cr, i, q);
        assert!(o.plan.is_some());
        // State unchanged, so the next cycle presents the same plan.
        assert_eq!(d.state, q.state);
    }

    // ---------------- Tier 2 ----------------

    /// A message shorter than the MTU is a single ONLY packet.
    #[test]
    fn a_short_message_is_one_only_packet() {
        let p = plans(16, Pmtu::B256, 1);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].opcode, WriteOpcode::Only);
        assert_eq!(p[0].psn, bits::<24>(1));
        assert_eq!(p[0].payload_bytes, bits::<13>(16));
        assert_eq!(p[0].padcount, bits::<2>(0));
        assert!(p[0].carries_reth);
        assert_eq!(p[0].reth.dma_len, bits::<32>(16));
        assert_eq!(p[0].reth.va, bits::<64>(VA));
    }

    /// A zero-length message is a legal single packet that moves no
    /// data. The natural implementation emits nothing at all, so this
    /// is worth pinning.
    #[test]
    fn a_zero_length_message_is_still_one_packet() {
        let p = plans(0, Pmtu::B1024, 5);
        assert_eq!(p.len(), 1, "a zero-length WRITE is one ONLY packet");
        assert_eq!(p[0].opcode, WriteOpcode::Only);
        assert_eq!(p[0].payload_bytes, bits::<13>(0));
        assert_eq!(p[0].padcount, bits::<2>(0));
        assert_eq!(p[0].reth.dma_len, bits::<32>(0));
    }

    /// A message exactly one MTU long is still ONLY, not FIRST+LAST.
    /// The boundary is `<=`, and an off-by-one here doubles the packet
    /// count for every aligned message.
    #[test]
    fn a_message_of_exactly_one_mtu_is_one_packet() {
        let p = plans(256, Pmtu::B256, 0);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].opcode, WriteOpcode::Only);
        assert_eq!(p[0].payload_bytes, bits::<13>(256));
    }

    /// One byte over the MTU is two packets, and the second is LAST.
    #[test]
    fn one_byte_over_the_mtu_is_first_then_last() {
        let p = plans(257, Pmtu::B256, 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].opcode, WriteOpcode::First);
        assert_eq!(p[0].payload_bytes, bits::<13>(256));
        assert_eq!(p[1].opcode, WriteOpcode::Last);
        assert_eq!(p[1].payload_bytes, bits::<13>(1));
        assert_eq!(p[1].padcount, bits::<2>(3));
        assert!(!p[1].carries_reth);
    }

    /// The segmenter's invariants, over a span of lengths and every
    /// MTU: the payload bytes sum to the message, the PSNs are
    /// consecutive, exactly one packet carries the RETH, and only the
    /// last packet pads.
    #[test]
    fn segmentation_invariants_hold_across_lengths_and_mtus() {
        for pmtu in [Pmtu::B256, Pmtu::B512, Pmtu::B1024] {
            let mtu = pmtu_bytes(pmtu).raw();
            for length in [0u128, 1, 3, 4, 255, 256, 257, 600, 1000, 2049, 4096] {
                if length > 8 * mtu {
                    continue; // beyond the stimulus length
                }
                let p = plans(length, pmtu, 0x20);
                let total: u128 = p.iter().map(|x| x.payload_bytes.raw()).sum();
                assert_eq!(
                    total, length,
                    "pmtu {mtu}, length {length}: payload bytes sum to {total}"
                );
                assert_eq!(
                    p.len(),
                    (length.div_ceil(mtu)).max(1) as usize,
                    "pmtu {mtu}, length {length}: wrong packet count"
                );
                for (n, pk) in p.iter().enumerate() {
                    assert_eq!(
                        pk.psn,
                        bits::<24>(0x20 + n as u128),
                        "pmtu {mtu}, length {length}: PSN not consecutive"
                    );
                    assert_eq!(
                        pk.carries_reth,
                        n == 0,
                        "pmtu {mtu}, length {length}: RETH on packet {n}"
                    );
                    let is_last = n + 1 == p.len();
                    if !is_last {
                        assert_eq!(
                            pk.payload_bytes.raw(),
                            mtu,
                            "pmtu {mtu}, length {length}: non-final packet not full"
                        );
                        assert_eq!(
                            pk.padcount,
                            bits::<2>(0),
                            "pmtu {mtu}, length {length}: non-final packet padded"
                        );
                    }
                    assert_eq!(
                        pk.reth.dma_len,
                        bits::<32>(length),
                        "RETH dma_len should be the whole message, not this packet"
                    );
                }
            }
        }
    }

    /// Middle packets appear for a message more than two MTUs long, and
    /// nothing else does.
    #[test]
    fn a_long_message_is_first_middles_then_last() {
        let p = plans(1000, Pmtu::B256, 0);
        assert_eq!(p.len(), 4);
        let ops: Vec<WriteOpcode> = p.iter().map(|x| x.opcode).collect();
        assert_eq!(
            ops,
            vec![
                WriteOpcode::First,
                WriteOpcode::Middle,
                WriteOpcode::Middle,
                WriteOpcode::Last
            ]
        );
        assert_eq!(p[3].payload_bytes, bits::<13>(1000 - 768));
    }

    // ---------------- VM cross-validation ----------------

    #[test]
    fn pad_for_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|n| (bits::<13>(n),));
        test_kernel_vm_and_verilog_synchronous::<pad_for, _, _, _>(pad_for, inputs)?;
        Ok(())
    }

    #[test]
    fn write_opcode_for_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = [(true, true), (true, false), (false, true), (false, false)].into_iter();
        test_kernel_vm_and_verilog_synchronous::<write_opcode_for, _, _, _>(
            write_opcode_for,
            inputs,
        )?;
        Ok(())
    }

    #[test]
    fn pmtu_bytes_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = [
            Pmtu::B256,
            Pmtu::B512,
            Pmtu::B1024,
            Pmtu::B2048,
            Pmtu::B4096,
        ]
        .into_iter()
        .map(|m| (m,));
        test_kernel_vm_and_verilog_synchronous::<pmtu_bytes, _, _, _>(pmtu_bytes, inputs)?;
        Ok(())
    }

    // ---------------- Tier 3 ----------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = WriteSegmenter::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [197:0] i, output wire [172:0] o);
               wire [326:0] od;
               wire [153:0] d;
               wire [153:0] q;
               assign o = od[172:0];
               top_state c0(.clock_reset(clock_reset), .i(d[153:0]), .o(q[153:0]));
               assign d = od[326:173];
               assign od = kernel_write_segmenter(clock_reset, i, q);
               function [326:0] kernel_write_segmenter(input reg [1:0] arg_0, input reg [197:0] arg_1, input reg [153:0] arg_2);
                     reg [153:0] r0;
                     reg [75:0] r1;
                     reg [197:0] r2;
                     reg [2:0] r3;
                     reg [12:0] r4;
                     reg [31:0] r5;
                     reg [31:0] r6;
                     reg [0:0] r7;
                     reg [31:0] r8;
                     reg [31:0] r9;
                     reg [0:0] r10;
                     reg [0:0] r11;
                     reg [2:0] r12;
                     reg [2:0] r13;
                     reg [2:0] r14;
                     reg [63:0] r15;
                     // reth
                     reg [127:0] r16;
                     reg [75:0] r17;
                     reg [31:0] r18;
                     // reth
                     reg [127:0] r19;
                     reg [31:0] r20;
                     // reth
                     reg [127:0] r21;
                     // plan
                     reg [170:0] r22;
                     reg [23:0] r23;
                     // plan
                     reg [170:0] r24;
                     reg [12:0] r25;
                     // plan
                     reg [170:0] r26;
                     reg [12:0] r27;
                     reg [12:0] r28;
                     reg [2:0] r29;
                     reg [2:0] r30;
                     reg [2:0] r31;
                     reg [1:0] r32;
                     // plan
                     reg [170:0] r33;
                     reg [0:0] r34;
                     // plan
                     reg [170:0] r35;
                     // plan
                     reg [170:0] r36;
                     reg [0:0] r37;
                     reg [171:0] r38;
                     reg [170:0] r39;
                     reg [171:0] r40;
                     // o
                     reg [172:0] r41;
                     reg [0:0] r42;
                     // o
                     reg [172:0] r43;
                     reg [0:0] r44;
                     reg [0:0] r45;
                     reg [0:0] r46;
                     // st
                     reg [153:0] r47;
                     reg [23:0] r48;
                     reg [23:0] r49;
                     // st
                     reg [153:0] r50;
                     reg [31:0] r51;
                     reg [31:0] r52;
                     // st
                     reg [153:0] r53;
                     reg [0:0] r54;
                     // st
                     reg [153:0] r55;
                     // st
                     reg [153:0] r56;
                     reg [0:0] r57;
                     // st
                     reg [153:0] r58;
                     // st
                     reg [153:0] r59;
                     reg [95:0] r60;
                     reg [31:0] r61;
                     // st
                     reg [153:0] r62;
                     reg [23:0] r63;
                     // st
                     reg [153:0] r64;
                     reg [95:0] r65;
                     reg [63:0] r66;
                     // st
                     reg [153:0] r67;
                     reg [95:0] r68;
                     reg [31:0] r69;
                     // st
                     reg [153:0] r70;
                     // st
                     reg [153:0] r71;
                     // d
                     reg [153:0] r72;
                     reg [0:0] r73;
                     reg [1:0] r74;
                     reg [0:0] r75;
                     // d
                     reg [153:0] r76;
                     // d
                     reg [153:0] r77;
                     // d
                     reg [153:0] r78;
                     // d
                     reg [153:0] r79;
                     // d
                     reg [153:0] r80;
                     // d
                     reg [153:0] r81;
                     // d
                     reg [153:0] r82;
                     // o
                     reg [172:0] r83;
                     // o
                     reg [172:0] r84;
                     // d
                     reg [153:0] r85;
                     // o
                     reg [172:0] r86;
                     reg [326:0] r87;
                     localparam l0 = 3'b000;
                     localparam l1 = 13'b0000100000000;
                     localparam l2 = 3'b001;
                     localparam l3 = 13'b0001000000000;
                     localparam l4 = 3'b010;
                     localparam l5 = 13'b0010000000000;
                     localparam l6 = 3'b011;
                     localparam l7 = 13'b0100000000000;
                     localparam l8 = 3'b100;
                     localparam l9 = 13'b1000000000000;
                     localparam l10 = 3'b010;
                     localparam l11 = 3'b001;
                     localparam l12 = 3'b000;
                     localparam l13 = 3'b100;
                     localparam l14 = 128'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l15 = 171'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l16 = 13'b0000000000011;
                     localparam l17 = 3'b100;
                     localparam l18 = 3'b011;
                     localparam l19 = 3'b000;
                     localparam l20 = 1'b1;
                     localparam l21 = 3'b100;
                     localparam l22 = 1'b1;
                     localparam l23 = 3'b101;
                     localparam l24 = 1'b1;
                     localparam l25 = 3'b001;
                     localparam l26 = 1'b0;
                     localparam l27 = 3'b010;
                     localparam l28 = 1'b0;
                     localparam l29 = 3'b011;
                     localparam l30 = 1'b0;
                     localparam l31 = 1'b1;
                     localparam l32 = 172'b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000;
                     localparam l33 = 173'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l34 = 1'b0;
                     localparam l35 = 24'b000000000000000000000001;
                     localparam l36 = 1'b1;
                     localparam l37 = 1'b1;
                     localparam l38 = 154'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l39 = 154'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l40 = 1'b0;
                     localparam l41 = 1'b0;
                     localparam l42 = 32'b00000000000000000000000000000000;
                     localparam l43 = 24'b000000000000000000000000;
                     localparam l44 = 64'b0000000000000000000000000000000000000000000000000000000000000000;
                     localparam l45 = 32'b00000000000000000000000000000000;
                     localparam l46 = 172'b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000;
                     localparam l47 = 1'b0;
                     begin
                        r74 = arg_0;
                        r2 = arg_1;
                        r0 = arg_2;
                        r1 = r2[172:97];
                        r3 = r1[27:25];
                        case (r3)
                           3'b000 : r4 = l1;
                           3'b001 : r4 = l3;
                           3'b010 : r4 = l5;
                           3'b011 : r4 = l7;
                           3'b100 : r4 = l9;
                        endcase
                        r5 = {{19{1'b0}}, r4};
                        r6 = r0[33:2];
                        r7 = r6 <= r5;
                        r8 = r0[33:2];
                        r9 = r7 ? r8 : r5;
                        r10 = r0[1:1];
                        r11 = r10 & r7;
                        r12 = r7 ? l10 : l11;
                        r13 = r10 ? l12 : r12;
                        r14 = r11 ? l13 : r13;
                        r15 = r0[121:58];
                        r16 = l14;
                        r16[63:0] = r15;
                        r17 = r2[172:97];
                        r18 = r17[59:28];
                        r19 = r16;
                        r19[95:64] = r18;
                        r20 = r0[153:122];
                        r21 = r19;
                        r21[127:96] = r20;
                        r22 = l15;
                        r22[2:0] = r14;
                        r23 = r0[57:34];
                        r24 = r22;
                        r24[26:3] = r23;
                        r25 = r9[12:0];
                        r26 = r24;
                        r26[39:27] = r25;
                        r27 = r9[12:0];
                        r28 = r27 & l16;
                        r29 = r28[2:0];
                        r30 = l17 - r29;
                        r31 = r30 & l18;
                        r32 = r31[1:0];
                        r33 = r26;
                        r33[41:40] = r32;
                        case (r14)
                           3'b000 : r34 = l20;
                           3'b100 : r34 = l22;
                           3'b101 : r34 = l24;
                           3'b001 : r34 = l26;
                           3'b010 : r34 = l28;
                           3'b011 : r34 = l30;
                        endcase
                        r35 = r33;
                        r35[42:42] = r34;
                        r36 = r35;
                        r36[170:43] = r21;
                        r37 = r0[0:0];
                        r39 = r36[170:0];
                        r38 = {l31, r39};
                        r40 = r37 ? r38 : l32;
                        r41 = l33;
                        r41[171:0] = r40;
                        r42 = r0[0:0];
                        r43 = r41;
                        r43[172:172] = r42;
                        r44 = r0[0:0];
                        r45 = r2[197:197];
                        r46 = r44 & r45;
                        r47 = r0;
                        r47[1:1] = l34;
                        r48 = r47[57:34];
                        r49 = r48 + l35;
                        r50 = r47;
                        r50[57:34] = r49;
                        r51 = r50[33:2];
                        r52 = r51 - r9;
                        r53 = r50;
                        r53[33:2] = r52;
                        r54 = ~r7;
                        r55 = r53;
                        r55[0:0] = r54;
                        r56 = r46 ? r55 : r0;
                        r57 = r2[0:0];
                        r58 = r56;
                        r58[0:0] = l36;
                        r59 = r58;
                        r59[1:1] = l37;
                        r60 = r2[96:1];
                        r61 = r60[95:64];
                        r62 = r59;
                        r62[33:2] = r61;
                        r63 = r2[196:173];
                        r64 = r62;
                        r64[57:34] = r63;
                        r65 = r2[96:1];
                        r66 = r65[63:0];
                        r67 = r64;
                        r67[121:58] = r66;
                        r68 = r2[96:1];
                        r69 = r68[95:64];
                        r70 = r67;
                        r70[153:122] = r69;
                        r71 = r57 ? r70 : r56;
                        r72 = l38;
                        r72[153:0] = r71;
                        r73 = r74[1:1];
                        r75 = |r73;
                        r76 = r72;
                        r76[153:0] = l39;
                        r77 = r76;
                        r77[0:0] = l40;
                        r78 = r77;
                        r78[1:1] = l41;
                        r79 = r78;
                        r79[33:2] = l42;
                        r80 = r79;
                        r80[57:34] = l43;
                        r81 = r80;
                        r81[121:58] = l44;
                        r82 = r81;
                        r82[153:122] = l45;
                        r83 = r43;
                        r83[171:0] = l46;
                        r84 = r83;
                        r84[172:172] = l47;
                        r85 = r75 ? r82 : r72;
                        r86 = r75 ? r84 : r43;
                        r87 = {r85, r86};
                        kernel_write_segmenter = r87;
                     end
               endfunction
            endmodule
            module top_state(input wire [1:0] clock_reset, input wire [153:0] i, output reg [153:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 154'b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 154'b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // ---------------- Tier 4 ----------------

    fn stimulus() -> Vec<In> {
        let idle = In {
            start: false,
            msg: MessageRequest {
                va: bits(VA),
                length: bits(600),
            },
            ctx: ctx(Pmtu::B256),
            psn_start: bits(0x10),
            ready: true,
        };
        let mut stream = vec![In {
            start: true,
            ..idle
        }];
        for n in 0..8 {
            // Exercise backpressure: hold ready low on one cycle.
            stream.push(In {
                ready: n != 2,
                ..idle
            });
        }
        stream
    }

    #[test]
    fn test_segmenter_hdl_works() -> miette::Result<()> {
        let uut = WriteSegmenter::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    // ---------------- Tier 5 ----------------

    #[test]
    fn test_segmenter_trace() -> miette::Result<()> {
        let uut = WriteSegmenter::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("segmenter");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["5aad28ea1762ba4f81b1a51409e8faa9715ac746ace9949e18826524557f38ba"];
        let digest = vcd.dump_to_file(root.join("segmenter.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
