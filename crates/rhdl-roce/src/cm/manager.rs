//! The connection manager: receive a setup datagram, apply it, reply.
//!
//! This is the widget that makes a queue pair come into existence from
//! the network rather than from parameters compiled in. It receives a
//! [`super::message`] datagram byte by byte, validates it, emits the
//! queue-pair and memory-region writes it implies, and sends a reply
//! frame reporting what it did.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+ConnectionManager+-----+
            |                          |
  ?u8       |                          |  ?QpWrite
 +--------->| data            qp_write +--------->
            |                          |
  bool      |                          |  ?MrWrite
 +--------->| sof             mr_write +--------->
            |                          |
  bool      |                          |  ?u8
 +--------->| eof                reply +--------->
            |                          |
  Frame     |                          |  bool
 +--------->| cfg          reply_sof/eof+-------->
            |                          |
  bool      |                          |  CmStatus
 +--------->| ready             status +--------->
            +--------------------------+
")]
//!
//! # Internals
#![doc = badascii_doc::badascii!("
   request bytes                           reply bytes
        |                                       ^
        v                                       |
  +-----+------------------------------+        |
  | Idle -> Header -> Body -> Reply ---+--------+
  |   ^        |        |        |     |
  |   +--------+--------+--------+     |   82 bytes:
  |          (Drain on reject)         |   eth_byte | ipv4_byte
  +-----+------------------------------+   | udp_byte | cm_byte
        |
        +--> qp_write : configure the queue pair
        +--> mr_write : register the memory region
")]
//!
//! # Why this parses Ethernet, IPv4 and UDP again
//!
//! [`crate::rx::parse`] already walks those same 42 bytes, and this
//! widget repeats about forty lines of offset snapshots to do it a
//! second time. That is a real duplication and it is deliberate.
//!
//! The shared part belongs in `rhdl-fpga`'s `net` category —
//! `roce-v2-architecture.md` §6 proposes exactly that, with `ipv4.rs`
//! and `udp.rs` in it. But `architecture.md` requires sign-off before a
//! new top-level category is added to `rhdl-fpga`, and that sign-off
//! has not happened. Inventing the category unilaterally to avoid forty
//! duplicated lines would trade a small, visible duplication for an
//! architectural decision taken without review, which is the worse of
//! the two.
//!
//! So: duplicated on purpose, recorded here, and this is the second of
//! the two callers that makes the case for the category concrete.
//! Unifying them is a follow-up, not an oversight.
//!
//! # A trusted port
//!
//! A datagram accepted here reconfigures a queue pair and registers a
//! memory region at a local address the datagram itself names. **The
//! connection-manager port must therefore not be reachable from an
//! untrusted network.**
//!
//! Nothing in this crate enforces that; it is a deployment constraint,
//! and it is the same position a commodity RNIC is in with respect to
//! its PCIe configuration space. It is stated here, and in
//! [`super::message`], because a future change that exposes this port
//! more widely is a privilege escalation and should be read as one
//! rather than as a networking convenience.
//!
//! The checks that *are* enforced: the magic number
//! ([`super::message::CM_MAGIC`]), so a stray datagram on the port
//! cannot reconfigure anything; the UDP destination port; and the
//! refusal to act on a [`CmOpcode::Response`], so two of these widgets
//! pointed at each other cannot generate an unbounded exchange of
//! replies.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/cm_manager.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/cm_manager.md")]
//!
//! And the state machine:
#![doc = include_str!("../../doc/cm_manager_fsm.md")]

use super::message::{
    CM_MAGIC, CM_MSG_BYTES, CmMessage, CmOpcode, CmStatus, cm_byte, decode_cm_opcode,
    decode_cm_pmtu, decode_cm_transport, rel_index7,
};
use crate::frame::{FrameConfig, eth_byte, ipv4_byte, udp_byte};
use crate::qp::context::QpContext;
use crate::qp::mr::{MemoryRegion, MrWrite};
use crate::qp::table::QpWrite;
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Frame offset where the connection-manager payload begins: after
/// Ethernet, IPv4 and UDP.
pub const CM_PAYLOAD_AT: u128 = 42;

/// Total length of a reply frame: 14 Ethernet + 20 IPv4 + 8 UDP + 40
/// payload.
pub const CM_FRAME_BYTES: u128 = CM_PAYLOAD_AT + CM_MSG_BYTES;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
/// Where the connection manager is.
pub enum CmPhase {
    #[default]
    /// Between datagrams.
    Idle,
    /// Accumulating Ethernet, IPv4 and UDP headers, and checking them.
    Header,
    /// Accumulating the 40-byte connection-manager payload.
    Body,
    /// The datagram is not ours, or is malformed; consume the rest of
    /// it. No reply is sent, because a datagram we could not identify
    /// is not one we should answer.
    Drain,
    /// Emitting the reply frame.
    Reply,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
#[doc(hidden)]
/// The manager's registers, bundled per CLAUDE.md §3.1.
pub struct CmRegs {
    /// Offset of the next byte expected in the request.
    pub offset: Bits<16>,
    /// Rolling accumulator. 64 bits is the widest single field — the
    /// region's virtual address.
    pub acc: Bits<64>,
    /// The request, as decoded so far.
    pub msg: CmMessage,
    /// Whether the magic number matched.
    pub magic_ok: bool,
    /// Whether every coded field decoded to something we implement.
    pub params_ok: bool,
    /// The peer's MAC, learned from the request so the reply can be
    /// addressed without any configured knowledge of the peer.
    pub peer_mac: Bits<48>,
    /// The peer's IPv4 address, likewise.
    pub peer_ip: Bits<32>,
    /// The peer's UDP source port, which the reply is sent back to.
    pub peer_port: Bits<16>,
    /// Set once the datagram has been positively identified as ours —
    /// IPv4, UDP, and the manager's own destination port.
    ///
    /// Load-bearing, and not the same thing as "no check has failed".
    /// The port is at offset 37, so a frame shorter than that has
    /// *passed* every check that ran while never having been shown to
    /// be addressed here. Replying to one would make this widget an
    /// amplifier: a one-byte frame in, an 82-byte reply out.
    pub identified: bool,
    /// The status the reply will carry.
    pub status: CmStatus,
    /// Index of the next reply byte.
    pub reply_index: Bits<7>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = CmPhase, allow_implicit)]
/// The connection manager. See the [module docs](self).
pub struct ConnectionManager {
    state: dff::DFF<CmPhase>,
    regs: dff::DFF<CmRegs>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`ConnectionManager`].
pub struct In {
    /// The next request byte, or `None` when the link is idle.
    pub data: Option<Bits<8>>,
    /// This byte is the first of a frame.
    pub start_of_frame: bool,
    /// This byte is the last of a frame.
    pub end_of_frame: bool,
    /// This interface's MAC and IPv4 address, for the reply's source
    /// fields. The reply's *destination* fields are learned from the
    /// request, so nothing about the peer need be configured.
    pub cfg: FrameConfig,
    /// The UDP port this manager listens on.
    /// [`super::message::CM_UDP_PORT`] is the conventional value.
    pub port: Bits<16>,
    /// The reply sink can accept a byte this cycle.
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [`ConnectionManager`].
pub struct Out {
    /// Configure or tear down a queue pair. Asserted for one cycle.
    pub qp_write: Option<QpWrite>,
    /// Register or deregister a memory region. Asserted for one cycle,
    /// in the same cycle as `qp_write`, so the two cannot be applied
    /// half-way.
    pub mr_write: Option<MrWrite>,
    /// A reply frame byte, or `None`. Held until `ready`.
    pub reply: Option<Bits<8>>,
    /// This reply byte is the first of its frame.
    pub reply_sof: bool,
    /// This reply byte is the last of its frame.
    pub reply_eof: bool,
    /// The status of the most recent request, valid from `done`.
    pub status: CmStatus,
    /// A request was decided this cycle.
    pub done: bool,
    /// A request or reply is in progress.
    pub busy: bool,
    /// The phase, for debug and waveform reading.
    pub phase: CmPhase,
}

impl SynchronousIO for ConnectionManager {
    type I = In;
    type O = Out;
    type Kernel = cm_manager;
}

#[kernel]
/// Byte `index` of the reply frame.
///
/// The reply is addressed back to whoever sent the request: its
/// destination MAC, IP and port come from the request's source fields,
/// which is why a reply needs nothing configured about the peer.
pub fn cm_reply_byte(cfg: FrameConfig, msg: CmMessage, index: Bits<7>) -> Bits<8> {
    let total_len = bits::<16>(CM_FRAME_BYTES - 14);
    let udp_len = bits::<16>(CM_MSG_BYTES + 8);
    let mut out = bits::<8>(0);
    if index < bits(14) {
        out = eth_byte(cfg, rel_index7(index, bits(0), bits(13)).resize::<4>());
    } else if index < bits(34) {
        out = ipv4_byte(
            cfg,
            total_len,
            rel_index7(index, bits(14), bits(33)).resize::<5>(),
        );
    } else if index < bits(42) {
        out = udp_byte(
            cfg,
            udp_len,
            rel_index7(index, bits(34), bits(41)).resize::<3>(),
        );
    } else if index < bits(CM_FRAME_BYTES) {
        out = cm_byte(
            msg,
            rel_index7(index, bits(42), bits(CM_FRAME_BYTES - 1)).resize::<6>(),
        );
    }
    out
}

#[kernel]
// `if let` and match guards are not in the kernel-accepted subset, so
// the single-arm matches below are the only spelling available. Same
// reasoning as `crate::rx::endpoint`.
#[allow(clippy::single_match, clippy::collapsible_match)]
/// Kernel for [`ConnectionManager`].
pub fn cm_manager(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();
    d.state = q.state;
    let mut r = q.regs;

    o.qp_write = None;
    o.mr_write = None;
    o.reply = None;
    o.reply_sof = false;
    o.reply_eof = false;
    o.status = q.regs.status;
    o.done = false;
    o.busy = q.state != CmPhase::Idle;
    o.phase = q.state;

    let byte = match i.data {
        Some(b) => b,
        None => bits(0),
    };
    let valid = match i.data {
        Some(_b) => true,
        None => false,
    };

    // ---------------------------------------------------- the reply
    //
    // Emitted first in the kernel so the request path below can
    // override `d.state` on a new start-of-frame, which is what lets a
    // request arrive while a reply is still going out.
    if q.state == CmPhase::Reply {
        // The reply's destination is the request's source.
        let mut rc = i.cfg;
        rc.dst_mac = q.regs.peer_mac;
        rc.dst_ip = q.regs.peer_ip;
        rc.src_port = i.port;
        rc.dst_port = q.regs.peer_port;

        let mut m = q.regs.msg;
        m.opcode = CmOpcode::Response;
        m.status = q.regs.status;

        o.reply = Some(cm_reply_byte(rc, m, q.regs.reply_index));
        o.reply_sof = q.regs.reply_index == bits(0);
        o.reply_eof = q.regs.reply_index == bits(CM_FRAME_BYTES - 1);
        if i.ready {
            if q.regs.reply_index == bits(CM_FRAME_BYTES - 1) {
                r.reply_index = bits(0);
                d.state = CmPhase::Idle;
            } else {
                r.reply_index = q.regs.reply_index + bits(1);
            }
        }
    }

    // -------------------------------------------------- the request
    //
    // Request bytes are taken only while a request is actually being
    // parsed. `q.state != Idle` would have been the obvious condition
    // and is wrong: it also admits bytes arriving during `Reply`, which
    // would be folded into the request registers at whatever offset was
    // left over and could manufacture a second decision from one
    // datagram. A real link always marks a frame's start, so this never
    // fired in practice -- which is exactly why it is worth closing
    // rather than relying on.
    let parsing =
        q.state == CmPhase::Header || q.state == CmPhase::Body || q.state == CmPhase::Drain;
    let taken = valid && (i.start_of_frame || parsing);
    if taken {
        if i.start_of_frame {
            // A new request abandons whatever was in progress, reply
            // included. A peer that retries before we have finished
            // answering gets the answer to the retry.
            r = CmRegs::default();
            r.magic_ok = false;
            r.params_ok = true;
        }
        let cur = r.offset;
        let acc = (r.acc << 8) | byte.resize::<64>();
        r.acc = acc;
        r.offset = cur + bits(1);

        let mut reject = q.state == CmPhase::Drain && !i.start_of_frame;

        // --- Ethernet, IPv4 and UDP. See the module docs on why this
        // is a second copy of these offsets.
        if cur == bits(11) {
            r.peer_mac = (acc & bits(0xFFFF_FFFF_FFFF)).resize::<48>();
        }
        if cur == bits(13) && (acc & bits(0xFFFF)) != bits(0x0800) {
            reject = true;
        }
        if cur == bits(14) && (acc & bits(0xFF)) != bits(0x45) {
            reject = true;
        }
        if cur == bits(23) && (acc & bits(0xFF)) != bits(17) {
            reject = true;
        }
        if cur == bits(29) {
            r.peer_ip = (acc & bits(0xFFFF_FFFF)).resize::<32>();
        }
        if cur == bits(35) {
            r.peer_port = (acc & bits(0xFFFF)).resize::<16>();
        }
        if cur == bits(37) {
            if (acc & bits(0xFFFF)) != i.port.resize::<64>() {
                reject = true;
            } else if !reject {
                // Everything before this point has passed *and* the
                // port is ours, so the datagram is now positively
                // identified and is one we will answer.
                r.identified = true;
            }
        }

        // --- the connection-manager payload.
        if cur == bits(45) {
            r.magic_ok = (acc & bits(0xFFFF_FFFF)) == bits(CM_MAGIC);
        }
        if cur == bits(46) {
            match decode_cm_opcode((acc & bits(0xFF)).resize::<8>()) {
                Some(op) => {
                    r.msg.opcode = op;
                }
                None => {
                    r.params_ok = false;
                }
            }
        }
        if cur == bits(48) {
            match decode_cm_transport((acc & bits(0xFF)).resize::<8>()) {
                Some(t) => {
                    r.msg.transport = t;
                }
                None => {
                    r.params_ok = false;
                }
            }
        }
        if cur == bits(49) {
            match decode_cm_pmtu((acc & bits(0xFF)).resize::<8>()) {
                Some(p) => {
                    r.msg.pmtu = p;
                }
                None => {
                    r.params_ok = false;
                }
            }
        }
        if cur == bits(52) {
            r.msg.qpn = (acc & bits(0xFF_FFFF)).resize::<24>();
        }
        if cur == bits(56) {
            r.msg.remote_qpn = (acc & bits(0xFF_FFFF)).resize::<24>();
        }
        if cur == bits(60) {
            r.msg.psn = (acc & bits(0xFF_FFFF)).resize::<24>();
        }
        if cur == bits(65) {
            r.msg.rkey = (acc & bits(0xFFFF_FFFF)).resize::<32>();
        }
        if cur == bits(73) {
            r.msg.va = acc;
        }
        if cur == bits(77) {
            r.msg.length = (acc & bits(0xFFFF_FFFF)).resize::<32>();
        }
        if cur == bits(81) {
            r.msg.local_base = (acc & bits(0xFFFF_FFFF)).resize::<32>();
        }

        // --- phases, as a match on the phase rather than one chain
        // over offsets.
        //
        // Same reason as `crate::rx::parse`: the FSM extractor cannot
        // narrow a target that does not depend on `q.state`, so a plain
        // offset chain made it report all twenty edges on five nodes --
        // `Idle -> Body` and `Idle -> Reply` among them, neither of
        // which can happen, since the only byte that leaves `Idle` is a
        // start-of-frame and that byte is offset zero. Matching on the
        // phase makes each arm name only its reachable targets.
        let after = cur + bits(1);
        match q.state {
            CmPhase::Idle => d.state = CmPhase::Header,
            CmPhase::Header => {
                if reject {
                    d.state = CmPhase::Drain;
                } else if after < bits(CM_PAYLOAD_AT) {
                    d.state = CmPhase::Header;
                } else {
                    d.state = CmPhase::Body;
                }
            }
            // No `Drain` from here: every check sits at an offset
            // below 42, and `reject` is only carried forward from a
            // `Drain` we are already in -- so it cannot become true
            // once the payload has started.
            CmPhase::Body => d.state = CmPhase::Body,
            CmPhase::Drain => d.state = CmPhase::Drain,
            // Only reachable on a start-of-frame, handled just below.
            CmPhase::Reply => d.state = CmPhase::Header,
        }
        if i.start_of_frame {
            d.state = CmPhase::Header;
        }

        // --- the decision, at the end of the request.
        if i.end_of_frame {
            // Constrained on the phase as well as on the data. The
            // data half is what actually decides; naming the phases
            // lets the FSM extractor rule out a `Reply` edge from
            // `Idle` and from `Drain`, which keeps the generated
            // diagram tight as well as sound.
            let replying =
                (q.state == CmPhase::Header || q.state == CmPhase::Body) && !reject && r.identified;
            if !replying {
                // Not ours, or never shown to be ours. Say nothing.
                //
                // The `!identified` half is what stops a frame that
                // ended before the port check at offset 37 from being
                // answered -- see `CmRegs::identified`. Without it a
                // one-byte frame draws an 82-byte reply, which is an
                // amplification vector and was found by reading the
                // generated state diagram: it showed an `Idle -> Reply`
                // edge, which should not have existed.
                d.state = CmPhase::Idle;
            } else {
                let short = after < bits(CM_FRAME_BYTES);
                let mut status = CmStatus::Accepted;
                let mut apply = false;
                if short || !r.magic_ok {
                    status = CmStatus::BadMagic;
                } else if !r.params_ok {
                    status = CmStatus::BadParameter;
                } else if r.msg.opcode == CmOpcode::Response {
                    // A reply is not a request. Refusing to act on one
                    // is what stops two managers answering each other
                    // for ever.
                    status = CmStatus::BadOpcode;
                } else if r.msg.opcode == CmOpcode::Connect && r.msg.length == bits(0) {
                    status = CmStatus::BadRegion;
                } else {
                    // `status` is already `Accepted` from its
                    // initialiser; assigning it again here would make
                    // that initialiser dead.
                    apply = true;
                }

                r.status = status;
                o.status = status;
                o.done = true;

                if apply {
                    let connect = r.msg.opcode == CmOpcode::Connect;
                    let mut ctx = QpContext::dont_care();
                    ctx.dqpn = r.msg.remote_qpn;
                    ctx.transport = r.msg.transport;
                    ctx.pmtu = r.msg.pmtu;
                    ctx.rkey = r.msg.rkey;
                    ctx.pkey = bits(0xFFFF);

                    let mut reg = MemoryRegion::dont_care();
                    reg.rkey = r.msg.rkey;
                    reg.va_base = r.msg.va;
                    reg.length = r.msg.length;
                    reg.local_base = r.msg.local_base;
                    reg.remote_write = true;

                    let mut qw = QpWrite::dont_care();
                    qw.qpn = r.msg.qpn;
                    qw.ctx = if connect { Some(ctx) } else { None };
                    o.qp_write = Some(qw);

                    let mut mw = MrWrite::dont_care();
                    mw.rkey = r.msg.rkey;
                    mw.region = if connect { Some(reg) } else { None };
                    o.mr_write = Some(mw);
                }

                // A malformed-but-identifiable request still gets an
                // answer, because the sender is entitled to know why.
                r.reply_index = bits(0);
                d.state = CmPhase::Reply;
            }
            // Either way the request is over, so its accumulators are
            // cleared. `r.msg`, `r.peer_*` and `r.status` are kept --
            // the reply is built from them.
            r.offset = bits(0);
            r.acc = bits(0);
        }
    }

    if cr.reset.any() {
        d.state = CmPhase::Idle;
        r = CmRegs::default();
        o.qp_write = None;
        o.mr_write = None;
        o.reply = None;
        o.reply_sof = false;
        o.reply_eof = false;
        o.status = CmStatus::Accepted;
        o.done = false;
        o.busy = false;
        o.phase = CmPhase::Idle;
    }
    d.regs = r;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use rhdl_fpga::doc::render_fsm_diagram_markdown;

    fn cfg() -> FrameConfig {
        FrameConfig {
            dst_mac: bits(0),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_0132),
            dst_ip: bits(0),
            src_port: bits(0),
            dst_port: bits(0),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        }
    }

    fn msg() -> CmMessage {
        CmMessage {
            opcode: CmOpcode::Response,
            status: CmStatus::Accepted,
            transport: crate::headers::Transport::Uc,
            pmtu: crate::qp::context::Pmtu::B1024,
            qpn: bits(0x000111),
            remote_qpn: bits(0x000222),
            psn: bits(1),
            rkey: bits(0x0000_A5A5),
            va: bits(0x0000_7F00_1234_0000),
            length: bits(4096),
            local_base: bits(0x0002_0000),
        }
    }

    /// The reply frame is 82 bytes and its layers sit where they should.
    /// Read back independently rather than through the encoders, so a
    /// shifted boundary shows up.
    #[test]
    fn the_reply_frame_has_the_right_shape() {
        let f: Vec<u8> = (0..CM_FRAME_BYTES)
            .map(|k| cm_reply_byte(cfg(), msg(), bits(k)).raw() as u8)
            .collect();
        assert_eq!(f.len(), 82, "14 + 20 + 8 + 40");
        assert_eq!(&f[12..14], &[0x08, 0x00], "EtherType IPv4");
        assert_eq!(f[14], 0x45, "version 4, 5-word header");
        assert_eq!(
            u16::from_be_bytes([f[16], f[17]]),
            68,
            "IP total length: 20 + 8 + 40"
        );
        assert_eq!(f[23], 17, "protocol UDP");
        assert_eq!(u16::from_be_bytes([f[38], f[39]]), 48, "UDP length: 8 + 40");
        assert_eq!(&f[42..46], b"RCM1", "the payload's magic");
        assert_eq!(f[46], 3, "the Response opcode");
    }

    /// An index past the frame reads zero rather than wrapping into a
    /// field, which a kernel's all-arms evaluation makes worth pinning.
    #[test]
    fn a_reply_index_past_the_end_is_zero() {
        for k in CM_FRAME_BYTES..128 {
            assert_eq!(cm_reply_byte(cfg(), msg(), bits(k)).raw(), 0, "index {k}");
        }
    }

    #[test]
    fn cm_reply_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        let c = cfg();
        let m = msg();
        let inputs = (0u128..128).map(move |k| (c, m, bits::<7>(k)));
        test_kernel_vm_and_verilog_synchronous::<cm_reply_byte, _, _, _>(cm_reply_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn reset_returns_to_idle_and_emits_nothing() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q {
            state: CmPhase::Body,
            regs: CmRegs {
                offset: bits(60),
                ..Default::default()
            },
        };
        let i = In {
            data: Some(bits(0x42)),
            start_of_frame: false,
            end_of_frame: false,
            cfg: cfg(),
            port: bits(CM_UDP_PORT_FOR_TESTS),
            ready: true,
        };
        let (o, d) = cm_manager(cr, i, q);
        assert_eq!(d.state, CmPhase::Idle);
        assert_eq!(d.regs, CmRegs::default());
        assert_eq!(o.qp_write, None, "no configuration under reset");
        assert_eq!(o.mr_write, None);
        assert_eq!(o.reply, None);
        assert!(!o.done);
    }

    const CM_UDP_PORT_FOR_TESTS: u128 = super::super::message::CM_UDP_PORT;

    // --------------------------------------------- the FSM diagram

    /// Per CLAUDE.md §12 rule 14.
    #[test]
    fn test_fsm_diagram_matches_descriptor() {
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<ConnectionManager>()
            .expect("extracting transitions");
        let fresh = render_fsm_diagram_markdown::<ConnectionManager>(&transitions);
        let committed = include_str!("../../doc/cm_manager_fsm.md");
        assert_eq!(
            fresh.trim(),
            committed.trim(),
            "doc/cm_manager_fsm.md is stale -- re-run \
             `cargo run --example cm_manager -p rhdl-roce`"
        );
    }

    /// The phases and the edges between them, pinned by name.
    ///
    /// Fourteen edges, of which **two are over-approximations the
    /// extractor cannot rule out** and which the hardware cannot take:
    ///
    ///   - `Idle -> Reply` would need a frame that ended while still in
    ///     `Idle`, having been positively identified as ours. Identity
    ///     is established at offset 37, and a frame still in `Idle` has
    ///     consumed one byte, so it cannot have been.
    ///   - `Drain -> Reply` would need `reject` false in `Drain`, and
    ///     `reject` is carried forward from being in `Drain` at all.
    ///
    /// Both depend on register values rather than on `q.state`, so a
    /// walker over the RHIF cannot exclude them. That is the *sound*
    /// direction to be wrong in — the diagram claims the machine might
    /// do something it will not, never that it cannot do something it
    /// can — but it is worth naming so a reader is not left wondering.
    ///
    /// Everything else here is real, including the ones that look
    /// surprising: every parsing phase reaches `Header`, because a
    /// start-of-frame restarts from anywhere, and `Reply -> Header` is
    /// a peer retrying before its answer has finished going out.
    #[test]
    fn the_fsm_has_exactly_the_edges_the_protocol_implies() {
        let t = rhdl::core::fsm::extract_widget_transitions_strict::<ConnectionManager>()
            .expect("extracting transitions");
        let name = ["Idle", "Header", "Body", "Drain", "Reply"];
        let mut edges: Vec<String> = t
            .iter()
            .filter(|x| x.source_index != x.target_index)
            .map(|x| format!("{} -> {}", name[x.source_index], name[x.target_index]))
            .collect();
        edges.sort();
        edges.dedup();
        let expect = expect![[r#"
            Body -> Header
            Body -> Idle
            Body -> Reply
            Drain -> Header
            Drain -> Idle
            Drain -> Reply
            Header -> Body
            Header -> Drain
            Header -> Idle
            Header -> Reply
            Idle -> Header
            Idle -> Reply
            Reply -> Header
            Reply -> Idle
        "#]];
        expect.assert_eq(&format!("{}\n", edges.join("\n")));
    }

    // ------------------------------------------ Tiers 3, 4 and 5

    fn test_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In)>> {
        // A short, deliberately-not-ours datagram keeps the HDL
        // round-trip quick; the protocol behaviour is covered by
        // `tests/cm.rs`.
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
            cfg: cfg(),
            port: bits(CM_UDP_PORT_FOR_TESTS),
            ready: true,
        };
        let mut stream = vec![idle];
        let bytes: Vec<u8> = (0..60u8).collect();
        let last = bytes.len() - 1;
        for (k, b) in bytes.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        stream.push(idle);
        stream.into_iter().with_reset(1).clock_pos_edge(100)
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = ConnectionManager::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"));
        Ok(())
    }

    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut = ConnectionManager::default();
        let tb = uut
            .run(test_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut = ConnectionManager::default();
        let vcd = uut.run(test_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cm_manager");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["39d801287ec01742041946cbbffa60f32d3a4061849d498bc8507fc7bcb7a405"];
        let digest = vcd.dump_to_file(root.join("cm_manager.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
