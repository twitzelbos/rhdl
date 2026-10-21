//! Byte-serial parser for a received RoCEv2 frame.
//!
//! The receive direction's front door. One byte per cycle goes in, and
//! what comes out is the typed header records the rest of the receive
//! path works from, a verdict on whether the frame is ours to act on,
//! and the payload bytes separated from the headers that framed them.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+RxParser+-------------+
            |                        |
  ?u8       |                        |  ?RxHeaders
 +--------->| data           headers +--------->
            |                        |
  bool      |                        |  ?u8
 +--------->| sof            payload +--------->
            |                        |
  bool      |                        |  bool
 +--------->| eof               done +--------->
            |                        |
            |                        |  RxVerdict
            |                verdict +--------->
            |                        |
            |                        |  b32
            |                   icrc +--------->
            |                        |
            |                        |  RxPhase
            |                  phase +--------->
            +------------------------+
")]
//!
//! # What it decides, and when
//!
//! A frame is *ours* when every one of these holds — and the check for
//! each one fires at the earliest byte that can settle it, so a frame
//! addressed elsewhere is abandoned without parsing the rest of it:
//!
//! | Byte | Field | Required | Verdict if not |
//! |---:|---|---|---|
//! | 13 | EtherType | `0x0800` | [`RxVerdict::NotIpv4`] |
//! | 14 | Version / IHL | `0x45` | [`RxVerdict::Ipv4Unsupported`] |
//! | 17 | IP total length | `44 ..= 4160` | [`RxVerdict::BadLength`] |
//! | 23 | IP protocol | `17` | [`RxVerdict::NotUdp`] |
//! | 37 | UDP dest port | `4791` | [`RxVerdict::NotRocePort`] |
//! | 42 | BTH opcode | an RDMA WRITE we implement | [`RxVerdict::UnknownOpcode`] |
//!
//! IHL is required to be exactly 5 rather than merely at least 5. A
//! RoCEv2 sender must not emit IP options, so a frame carrying them is
//! either not from an RNIC or has been tampered with; parsing it would
//! mean a variable header length through the whole datapath, for a case
//! that should not occur. Rejecting is both safer and smaller.
//!
//! # Internals
//!
//! Every header field is a contiguous run of bytes, so there is exactly
//! one shift register. Each byte folds into a 128-bit accumulator, and
//! each field is snapshotted out of the accumulator's low bits at the
//! offset where that field ends. 128 bits is not a round number chosen
//! for comfort: the RETH is 16 bytes, the longest single field in the
//! frame, and it is what sets the width.
#![doc = badascii_doc::badascii!("
   byte
     |
     v
  +--+-----------------------+
  |   acc = acc<<8 | byte    |
  |        128 bits          |
  +--+-----------------------+
     |
     +--> ethertype    (off 13, checked)
     +--> version/IHL  (off 14, checked)
     +--> total_len    (off 17, checked)
     +--> protocol     (off 23, checked)
     +--> src_ip       (off 29)
     +--> dst_ip       (off 33)
     +--> src_port     (off 35)
     +--> dst_port     (off 37, checked)
     +--> opcode       (off 42, checked)
     +--> bth          (off 53)
     +--> reth         (off 69)
     +--> icrc         (low 32 bits, at end_of_frame)
")]
//!
//! That last line is worth dwelling on, because it removes a register
//! and an offset computation that would otherwise have to agree with
//! the length arithmetic. The ICRC is the frame's final four bytes, and
//! the accumulator keeps shifting through the payload, so at the last
//! byte its low 32 bits hold exactly those four bytes. Nothing has to
//! know *where* the ICRC starts.
//!
//! # Timing of the outputs
//!
//! `headers` is presented from the cycle the **first payload byte**
//! arrives, and held for the rest of the frame. That is not a
//! coincidence of the implementation; it is the earliest cycle it
//! could be presented, because the last header byte only reaches the
//! registers at the end of the cycle before. It is also exactly when
//! the placement path needs it: the RETH address must be known before
//! the first byte it addresses is handed over.
//!
//! `payload` carries its byte in the same cycle, so a consumer reads
//! `headers` and `payload` together and never has to buffer a byte
//! while waiting to learn where it goes.
//!
//! `verdict` and `icrc` are meaningful when `done` is set. **`done`
//! means the frame ended, not that it was good** — a frame whose
//! header checks all passed can still fail the ICRC comparison, which
//! is deliberately not this widget's job. So a consumer that writes
//! payload bytes as they arrive is writing *speculatively*, and must
//! commit on `done && verdict == Accept` plus an ICRC match. That is
//! the same bargain a real RNIC makes, and the alternative — buffering
//! a whole packet before placing any of it — costs a PMTU of memory
//! and the latency that motivated RDMA in the first place.
//!
//! # No backpressure, on purpose
//!
//! There is no `ready` input. A receiver cannot tell an Ethernet MAC to
//! wait, so an interface that implied it could would be a lie that
//! showed up as dropped frames at line rate. Everything here is
//! single-cycle per byte, so there is nothing to stall for.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/rx_parse.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rx_parse.md")]
//!
//! And the state machine the parser walks through a frame:
#![doc = include_str!("../../doc/rx_parse_fsm.md")]

use crate::frame::ROCE_UDP_PORT;
use crate::headers::{
    Bth, Reth, WriteKind, bth_unpack, carries_reth, decode_write_opcode, reth_unpack,
};
use crate::icrc::COVERAGE_START;
use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_fpga::net::udp_rx::{UdpHeader, UdpRx, UdpRxVerdict};

/// The smallest IP total length a RoCE RDMA WRITE can have: 20 bytes of
/// IPv4, 8 of UDP, 12 of BTH, no RETH, no payload, 4 of ICRC.
pub const MIN_IP_TOTAL_LEN: u128 = 44;

/// The largest we accept: the same overhead with a RETH, plus the
/// largest PMTU the IBTA defines (4096).
///
/// A bound is needed and not merely tidy. The frame-length arithmetic
/// adds 14 to this value in 16 bits, so an unbounded length could wrap
/// and make a truncated frame look complete.
pub const MAX_IP_TOTAL_LEN: u128 = 60 + 4096;

/// Frame offset of the first BTH byte — the opcode.
pub const OFF_BTH: u128 = 42;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Why the parser did or did not accept a frame.
///
/// Rejections are *reasons*, not one `bool`, because on a receive path
/// they are the only diagnostic available. A counter per variant tells
/// an operator whether the link is carrying someone else's broadcast
/// traffic, or whether a peer is sending opcodes this endpoint does not
/// implement — two situations with nothing in common that a single
/// `dropped` count would conflate.
pub enum RxVerdict {
    #[default]
    /// Every check this widget makes has passed so far. On `done` it
    /// means the headers are well-formed and the frame is addressed to
    /// RoCEv2 on this endpoint. It does **not** mean the ICRC matched.
    Accept,
    /// EtherType was not IPv4.
    NotIpv4,
    /// IPv4 version was not 4, or the header length was not 5 words.
    Ipv4Unsupported,
    /// IP total length is impossible for a RoCE RDMA WRITE.
    BadLength,
    /// IP protocol was not UDP.
    NotUdp,
    /// UDP destination port was not the RoCEv2 port.
    NotRocePort,
    /// The BTH opcode is not an RDMA WRITE of a transport service this
    /// crate implements.
    UnknownOpcode,
    /// The frame ended before the headers were complete, or its length
    /// disagreed with the IP total length field.
    Truncated,
    /// The Invariant CRC did not match the frame's contents.
    ///
    /// **Never produced by [`RxParser`]**, which reports the ICRC it
    /// received and compares nothing. It is produced by
    /// [`crate::rx::verify::RxVerify`], which composes the parser with
    /// the checksum engine. The variant lives here, in the one verdict
    /// type for the whole receive front end, so that a consumer has a
    /// single value to check rather than a verdict *and* a separate
    /// `icrc_ok` flag it could forget.
    IcrcMismatch,
    /// Bytes were still arriving after the IP total length said the
    /// frame should have ended.
    Overlong,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Everything the parser recovered from a frame's headers.
pub struct RxHeaders {
    /// The InfiniBand Base Transport Header.
    pub bth: Bth,
    /// The opcode, decoded into transport service and operation.
    pub kind: WriteKind,
    /// The RDMA Extended Transport Header. Meaningful only when
    /// `has_reth`; zero otherwise, since a packet without one does not
    /// restate the address.
    pub reth: Reth,
    /// Whether this packet carried a RETH. FIRST and ONLY do; MIDDLE
    /// and LAST do not, and inherit the address from the message's
    /// FIRST.
    pub has_reth: bool,
    /// The sender's IPv4 address.
    pub src_ip: Bits<32>,
    /// Our IPv4 address, as the sender addressed it. Not checked here —
    /// a frame reaching us has already been filtered by the MAC, and an
    /// endpoint with several addresses needs the value rather than a
    /// yes/no.
    pub dst_ip: Bits<32>,
    /// The sender's UDP source port. Carries no RoCE meaning; a sender
    /// varies it to spread traffic over a fabric's ECMP hashes.
    pub src_port: Bits<16>,
    /// The IPv4 total length field, as received.
    pub ip_total_len: Bits<16>,
    /// Payload bytes in this packet, derived from `ip_total_len` and
    /// whether a RETH is present.
    pub payload_len: Bits<13>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
/// Where in a frame the parser is.
pub enum RxPhase {
    #[default]
    /// Between frames. Only a `start_of_frame` leaves this state, which
    /// is what lets the parser resynchronise after a frame it did not
    /// see the beginning of.
    Idle,
    /// Inside the Ethernet, IPv4 and UDP headers, which
    /// [`rhdl_fpga::net::udp_rx::UdpRx`] is taking apart.
    Network,
    /// Accumulating the InfiniBand transport headers — the BTH, and the
    /// RETH when the opcode carries one.
    Transport,
    /// Passing payload bytes through.
    Payload,
    /// The last four bytes, which are the ICRC.
    Trailer,
    /// The frame was rejected; its remaining bytes are consumed and
    /// discarded so the parser is aligned for the next one.
    Drain,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
#[doc(hidden)]
/// The parser's own registers — the InfiniBand half only.
///
/// Much shorter than it was. The byte offset, the IPv4 total length and
/// the addresses and ports all moved into
/// [`rhdl_fpga::net::udp_rx::UdpRx`], which is the point of composing
/// it: this struct now holds what is specific to InfiniBand and
/// nothing else.
pub struct RxRegs {
    /// The transport-header accumulator, fed every UDP payload byte.
    ///
    /// 128 bits because the RETH is 16 bytes, the widest single field.
    /// Payload bytes keep shifting through after the snapshots below,
    /// which is deliberate — see `icrc` on how the trailer falls out of
    /// it for free.
    pub acc: Bits<128>,
    /// The packed BTH, snapshotted at payload index 11.
    pub bth: Bits<96>,
    /// The packed RETH, snapshotted at payload index 27 — and only when
    /// the opcode says there is one. For a MIDDLE or LAST that index is
    /// *payload*, and an ungated snapshot would load this with it.
    pub reth: Bits<128>,
    /// Whether the decoded opcode carries a RETH.
    pub has_reth: bool,
    /// The decoded opcode.
    pub kind: WriteKind,
    /// Sticky, and for RoCE-level rejections only; the network-level
    /// verdict comes from `UdpRx`.
    pub verdict: RxVerdict,
    /// Set once the transport headers are complete, which is what gates
    /// presenting `headers` and passing payload through.
    pub transport_valid: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = RxPhase, allow_implicit)]
/// Byte-serial RoCEv2 frame parser. See the [module docs](self).
pub struct RxParser {
    udp: UdpRx,
    state: dff::DFF<RxPhase>,
    extras: dff::DFF<RxRegs>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`RxParser`].
pub struct In {
    /// The next frame byte, or `None` when the link is idle. Framing is
    /// shaped to match [`crate::tx::RoceWriteTx`]'s output exactly, so
    /// the two compose into the loopback test of
    /// `roce-v2-architecture.md` Tier 10 with no adapter between them.
    pub data: Option<Bits<8>>,
    /// This byte is the first of a frame. Required to leave
    /// [`RxPhase::Idle`], and mid-frame it abandons the frame in
    /// progress and starts a new one.
    pub start_of_frame: bool,
    /// This byte is the last of a frame.
    pub end_of_frame: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [`RxParser`].
pub struct Out {
    /// The recovered headers, from the first payload byte's cycle to the
    /// end of the frame. `None` while still parsing, and for a frame
    /// that was rejected before its headers completed.
    pub headers: Option<RxHeaders>,
    /// A payload byte, in the cycle it arrives.
    pub payload: Option<Bits<8>>,
    /// The frame ended this cycle. Says nothing about whether it was
    /// good; read `verdict` for that.
    pub done: bool,
    /// The running verdict, final when `done`.
    pub verdict: RxVerdict,
    /// The ICRC as received, wire order. Meaningful when `done` and the
    /// verdict is [`RxVerdict::Accept`].
    pub icrc: Bits<32>,
    /// A frame is in progress.
    pub busy: bool,
    /// The byte to fold into the Invariant CRC, or `None` when this
    /// cycle's byte is outside the ICRC's covered range (or there is
    /// no byte).
    ///
    /// Deciding coverage needs the frame geometry, and that is why
    /// [`rhdl_fpga::net::udp_rx::UdpRx`] forwards every byte with its
    /// frame offset and exposes the IPv4 total length continuously
    /// rather than only inside its parsed header: the ICRC starts at
    /// frame offset 14, long before the header block is complete.
    /// Emitting the feed, the index and `frame_start` together means a
    /// widget composing this with [`crate::icrc::Icrc`] wires three
    /// signals across and has no relative timing to reason about.
    /// [`crate::rx::verify::RxVerify`] is that widget.
    pub icrc_feed: Option<Bits<8>>,
    /// The frame offset of this cycle's byte, which is what selects the
    /// ICRC's field masking. Zero when there is no byte.
    pub frame_index: Bits<16>,
    /// A frame began this cycle -- the cycle to re-seed a checksum on.
    pub frame_start: bool,
    /// Which phase the parser is in *for the byte on `data` this
    /// cycle* — the registered phase, not the one being moved to.
    ///
    /// An output rather than private state because the phase is
    /// redundant with the byte offset, and redundant state that must
    /// agree with a counter is where parsers go wrong. Exposing it lets
    /// `the_phase_always_agrees_with_the_byte_offset` check the
    /// agreement on every byte of every frame in the corpus, which
    /// turns the redundancy into a tested invariant instead of a latent
    /// disagreement. It is also the signal worth looking at first in a
    /// waveform.
    pub phase: RxPhase,
}

impl SynchronousIO for RxParser {
    type I = In;
    type O = Out;
    type Kernel = rx_parse;
}

#[kernel]
/// Payload bytes implied by an IP total length.
///
/// Clamped at the subtraction *and* before narrowing to 13 bits,
/// because `resize` truncates rather than saturating: a `total_len`
/// below the overhead would otherwise wrap to a huge length, and one
/// above 8191 would silently lose its high bits. [`MAX_IP_TOTAL_LEN`]
/// makes the second case unreachable, and the mask makes it harmless
/// anyway.
pub fn payload_len_of(total_len: Bits<16>, has_reth: bool) -> Bits<13> {
    let overhead = if has_reth {
        bits::<16>(60)
    } else {
        bits::<16>(44)
    };
    let n = if total_len > overhead {
        total_len - overhead
    } else {
        bits::<16>(0)
    };
    (n & bits(0x1FFF)).resize::<13>()
}

#[kernel]
/// Translate a network-layer verdict into this parser's.
///
/// A plain renaming except for one case: a UDP port that is not ours is
/// [`RxVerdict::NotRocePort`], which says *which* port was expected in
/// a way the generic `PortMismatch` cannot.
pub fn verdict_of(v: UdpRxVerdict) -> RxVerdict {
    match v {
        UdpRxVerdict::Accept => RxVerdict::Accept,
        UdpRxVerdict::NotIpv4 => RxVerdict::NotIpv4,
        UdpRxVerdict::Ipv4Unsupported => RxVerdict::Ipv4Unsupported,
        UdpRxVerdict::BadLength => RxVerdict::BadLength,
        UdpRxVerdict::NotUdp => RxVerdict::NotUdp,
        UdpRxVerdict::PortMismatch => RxVerdict::NotRocePort,
        UdpRxVerdict::Truncated => RxVerdict::Truncated,
        UdpRxVerdict::Overlong => RxVerdict::Overlong,
    }
}

#[kernel]
// Three clippy suggestions here cannot be taken, all for the same
// reason: `#[kernel]` lowers `match` on an `Option`, not `Option`'s
// methods or sugar over them. `if let` is not accepted,
// `.unwrap_or_default()` is not accepted, and a match guard -- which
// `collapsible_match` would require -- is rejected outright with
// "Guards are not supported in rhdl kernel function match
// expressions". The explicit matches are the only spelling that
// compiles to hardware. Same reasoning, and the same allows, as
// `crate::endpoint` and `crate::rx::endpoint`.
#[allow(
    clippy::single_match,
    clippy::collapsible_match,
    clippy::manual_unwrap_or_default
)]
/// Kernel for [`RxParser`].
pub fn rx_parse(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();
    let mut r = q.extras;
    // The next phase is computed into a local and assigned to `d.state`
    // exactly once, at the very end.
    //
    // This is not style. `if cond { d.state = X }` lowers to a `Select`
    // over the *whole* `d`, so every field of it -- including `d.udp`,
    // the child's input -- inherits a dependency on whatever `cond`
    // reads. `cond` here reads `q.udp`, so conditional writes to
    // `d.state` make the combinational-reachability DRC see `udp`'s
    // output reaching `udp`'s own input and report a cycle through one
    // widget. There is no real loop; the select is what manufactures
    // it. Assigning each `d` field once, unconditionally, from a local
    // keeps the paths separable.
    let mut next_state = q.state;

    // ---- the network layers are UdpRx's job.
    d.udp.data = i.data;
    d.udp.start_of_frame = i.start_of_frame;
    d.udp.end_of_frame = i.end_of_frame;
    d.udp.expect_port = bits(ROCE_UDP_PORT);
    let u = q.udp;

    o.payload = None;
    o.done = false;
    o.icrc = bits(0);
    o.busy = u.busy;
    o.phase = q.state;
    o.headers = None;
    o.icrc_feed = None;
    o.frame_index = u.frame_index;
    o.frame_start = u.frame_start;

    // A network-level rejection outranks anything this layer has to
    // say: a frame that is not even IPv4 has no opcode to complain
    // about.
    // The parsed network header, or zeros before it is complete. Only
    // read once `transport_valid`, by which point UdpRx has presented
    // it -- the transport headers are inside the UDP payload, so the
    // network headers necessarily finished first.
    let net = match u.header {
        Some(h) => h,
        None => UdpHeader::default(),
    };
    let net_verdict = verdict_of(u.verdict);
    let mut verdict = if net_verdict != RxVerdict::Accept {
        net_verdict
    } else {
        r.verdict
    };
    o.verdict = verdict;

    // ---- the ICRC feed, which is why UdpRx forwards whole frames.
    //
    // Covered from COVERAGE_START up to but excluding the four-byte
    // ICRC field, which begins at `ip_total_len + 10`. The
    // `frame_index < 18` term is not redundant: `ip_total_len` is only
    // valid from the cycle after offset 17, so without it the three
    // bytes at offsets 14, 15 and 16 would be dropped from the
    // checksum.
    match u.byte {
        Some(b) => {
            if verdict == RxVerdict::Accept
                && u.frame_index >= bits(COVERAGE_START)
                && (u.frame_index < bits(18) || u.frame_index < u.ip_total_len + bits(10))
            {
                o.icrc_feed = Some(b);
            }
        }
        None => {}
    }

    // ---- RoCE's own length bound. UdpRx only rejects a length too
    // small to hold the headers it has already parsed; the smallest
    // *RoCE* frame is larger, and the largest is bounded by the PMTU.
    if u.frame_index >= bits(18)
        && (u.ip_total_len < bits(MIN_IP_TOTAL_LEN) || u.ip_total_len > bits(MAX_IP_TOTAL_LEN))
    {
        r.verdict = RxVerdict::BadLength;
        verdict = RxVerdict::BadLength;
        o.verdict = RxVerdict::BadLength;
    }

    // ---- present the headers, once the transport block is complete.
    if r.transport_valid && !u.frame_start {
        let mut h = RxHeaders::dont_care();
        h.bth = bth_unpack(r.bth);
        h.kind = r.kind;
        h.reth = reth_unpack(r.reth);
        h.has_reth = r.has_reth;
        h.src_ip = net.src_ip;
        h.dst_ip = net.dst_ip;
        h.src_port = net.src_port;
        h.ip_total_len = u.ip_total_len;
        h.payload_len = payload_len_of(u.ip_total_len, r.has_reth);
        o.headers = Some(h);
    }

    // ---- the transport headers, from UdpRx's payload stream.
    //
    // `saw_payload` drives the phase below rather than UdpRx's own
    // phase output: reaching across into a sibling's state machine
    // would couple the two in a way that a timing change in either
    // breaks silently, whereas the payload stream is the interface.
    let saw_payload = match u.payload {
        Some(_b) => true,
        None => false,
    };
    // Set when this cycle's byte is one of the four ICRC bytes rather
    // than payload. Hoisted out of the block below because the phase
    // machine needs it, and without it `Trailer` is unreachable -- a
    // dead state that the generated diagram would show as an island.
    let mut reached_trailer = false;
    //
    // Indices here are *payload*-relative, which is the other gain
    // from composing: the BTH ends at payload index 11 and the RETH at
    // 27, rather than at frame offsets 53 and 69 that have the network
    // headers' lengths baked into them.
    if u.frame_start {
        r = RxRegs::default();
    }
    match u.payload {
        Some(b) => {
            let k = u.payload_index;
            let acc = (r.acc << 8) | b.resize::<128>();
            if verdict == RxVerdict::Accept {
                r.acc = acc;
                if k == bits(0) {
                    match decode_write_opcode(b) {
                        Some(kind) => {
                            r.kind = kind;
                            r.has_reth = carries_reth(kind.op);
                        }
                        None => {
                            r.verdict = RxVerdict::UnknownOpcode;
                        }
                    }
                }
                if k == bits(11) {
                    r.bth = (acc & bits(0xFFFF_FFFF_FFFF_FFFF_FFFF_FFFF)).resize::<96>();
                }
                // Gated on `has_reth`: for a MIDDLE or LAST, payload
                // index 27 is payload, and an ungated snapshot would
                // load the RETH register with it. `has_reth` is false
                // there so a correct consumer ignores the field, but
                // leaving it zero makes forgetting that check write to
                // address zero rather than to an address made of
                // payload bytes.
                if k == bits(27) && r.has_reth {
                    r.reth = acc;
                }
                let transport_bytes = if r.has_reth {
                    bits::<16>(28)
                } else {
                    bits::<16>(12)
                };
                if k + bits(1) >= transport_bytes {
                    r.transport_valid = true;
                }
                // The payload proper: past the transport headers, and
                // before the four-byte ICRC trailer. The UDP payload is
                // `ip_total_len - 28` bytes, so its last non-trailer
                // index is `ip_total_len - 33`.
                let udp_payload = u.ip_total_len - bits(28);
                if r.transport_valid && k >= transport_bytes && k + bits(5) <= udp_payload {
                    o.payload = Some(b);
                }
                if r.transport_valid && k + bits(5) > udp_payload {
                    reached_trailer = true;
                }
            }
        }
        None => {}
    }

    // ---- phases, as a match on the phase rather than a chain over
    // offsets, so the FSM extractor can narrow each arm's targets.
    let rejected = r.verdict != RxVerdict::Accept || net_verdict != RxVerdict::Accept;
    // Gated on a byte actually having been taken. Without the gate the
    // `Idle` arm fires on every idle cycle and the parser sits in
    // `Network` before a frame has begun -- which is what
    // `the_phase_always_agrees_with_the_byte_offset` caught. The old
    // single-widget version had its phase machine inside the
    // byte-taken branch; composing UdpRx moved the byte test behind
    // `u.byte`, and the gate has to move with it.
    let took = match u.byte {
        Some(_b) => true,
        None => false,
    };
    if took {
        match q.state {
            RxPhase::Idle => next_state = RxPhase::Network,
            RxPhase::Network => {
                if rejected {
                    next_state = RxPhase::Drain;
                } else if saw_payload {
                    next_state = RxPhase::Transport;
                } else {
                    next_state = RxPhase::Network;
                }
            }
            RxPhase::Transport => {
                if rejected {
                    next_state = RxPhase::Drain;
                } else if reached_trailer {
                    // A zero-payload packet goes straight to the trailer.
                    next_state = RxPhase::Trailer;
                } else if r.transport_valid {
                    next_state = RxPhase::Payload;
                } else {
                    next_state = RxPhase::Transport;
                }
            }
            RxPhase::Payload => {
                if rejected {
                    next_state = RxPhase::Drain;
                } else if reached_trailer {
                    next_state = RxPhase::Trailer;
                } else {
                    next_state = RxPhase::Payload;
                }
            }
            RxPhase::Trailer => {
                if rejected {
                    next_state = RxPhase::Drain;
                } else {
                    next_state = RxPhase::Trailer;
                }
            }
            RxPhase::Drain => next_state = RxPhase::Drain,
        }
    }
    if u.frame_start {
        next_state = RxPhase::Network;
    }

    // ---- end of frame.
    if u.done {
        o.done = true;
        // RoCE's own verdict first, then the network layer's.
        //
        // That order looks backwards -- the lower layer is checked
        // first -- and it is right, because of *when* each is
        // detectable. A RoCE-level rejection can only be raised while
        // UdpRx is still accepting: `BadLength` at frame offset 18,
        // `UnknownOpcode` at offset 42. The network verdicts that
        // would otherwise mask them, `Truncated` and `Overlong`, are
        // only known at or near the end of the frame. So a frame
        // declaring a length too small for RoCE reports `BadLength`,
        // which says what is wrong, rather than the `Overlong` that
        // follows from it forty bytes later.
        if r.verdict != RxVerdict::Accept {
            o.verdict = r.verdict;
        } else if net_verdict != RxVerdict::Accept {
            o.verdict = net_verdict;
        } else if !r.transport_valid {
            // UdpRx accepted the datagram's extent, but there were not
            // enough payload bytes to hold a transport header.
            o.verdict = RxVerdict::Truncated;
        } else {
            o.verdict = RxVerdict::Accept;
            // The ICRC is the frame's last four bytes, and the
            // accumulator has kept shifting through the payload -- so
            // its low 32 bits are exactly those bytes. Nothing has to
            // know where the trailer starts.
            o.icrc = (r.acc & bits(0xFFFF_FFFF)).resize::<32>();
        }
        next_state = RxPhase::Idle;
        r = RxRegs::default();
    }

    if cr.reset.any() {
        next_state = RxPhase::Idle;
        // `d.udp` is deliberately NOT forced here.
        //
        // Two reasons, and the second one is a hard constraint. A
        // `Synchronous` child consumes the implicit `ClockReset` the
        // framework fans out to every sub-circuit, so `UdpRx` resets
        // itself and driving its inputs to idle is redundant.
        //
        // And it is not merely redundant: assigning `d.udp` inside this
        // conditional makes the whole of `d` a select between a
        // reset value and a value that depends on `q.udp` -- at which
        // point the combinational-reachability DRC sees `udp`'s output
        // reaching its own input and reports a cycle through one
        // widget. Leaving `d.udp` assigned exactly once, unconditionally
        // and only from `i`, is what keeps that path honest.
        r = RxRegs::default();
        o.headers = None;
        o.payload = None;
        o.done = false;
        o.verdict = RxVerdict::Accept;
        o.icrc = bits(0);
        o.busy = false;
        o.phase = RxPhase::Idle;
        o.icrc_feed = None;
        o.frame_index = bits(0);
        o.frame_start = false;
    }
    d.state = next_state;
    d.extras = r;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameConfig, bth_from_plan, header_byte, header_bytes};
    use crate::headers::{Transport, WriteOpcode};
    use crate::qp::context::{Pmtu, QpContext};
    use crate::qp::segmenter::PacketPlan;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use rhdl_fpga::doc::render_fsm_diagram_markdown;

    // ------------------------------------------------------- fixtures

    fn cfg() -> FrameConfig {
        FrameConfig {
            dst_mac: bits(0x000a_3501_0203),
            src_mac: bits(0xec0d_9aaa_bbcc),
            src_ip: bits(0xC0A8_010A),
            dst_ip: bits(0xC0A8_0132),
            src_port: bits(0x1234),
            dst_port: bits(4791),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        }
    }

    fn ctx() -> QpContext {
        QpContext {
            dqpn: bits(0x000111),
            transport: Transport::Uc,
            pmtu: Pmtu::B1024,
            rkey: bits(0x0000_A5A5),
            pkey: bits(0xFFFF),
        }
    }

    fn plan(op: WriteOpcode, payload: u128, pad: u128) -> PacketPlan {
        PacketPlan {
            opcode: op,
            psn: bits(1),
            payload_bytes: bits(payload),
            padcount: bits(pad),
            carries_reth: carries_reth(op),
            reth: Reth {
                va: bits(0x0000_7F00_1234_0000),
                rkey: bits(0x0000_A5A5),
                dma_len: bits(payload),
            },
        }
    }

    /// A complete, well-formed frame, built with the same pure
    /// functions the transmitter uses. The four trailer bytes are a
    /// placeholder — this widget reports the ICRC rather than checking
    /// it, so their value is immaterial here and recognisable in a dump.
    fn good_frame(op: WriteOpcode, payload: u128, pad: u128) -> Vec<u8> {
        let p = plan(op, payload, pad);
        let bth = bth_from_plan(ctx(), p, false);
        let n = header_bytes(p).raw() as usize;
        let mut f: Vec<u8> = (0..n)
            .map(|k| header_byte(cfg(), bth, p, bits(k as u128)).raw() as u8)
            .collect();
        f.extend((0..(payload + pad) as usize).map(|k| (0xA0 + k) as u8));
        f.extend([0xDE, 0xAD, 0xBE, 0xEF]);
        f
    }

    /// Outcome of driving one frame through the parser.
    struct Run {
        verdict: RxVerdict,
        dones: usize,
        payload: Vec<u8>,
        headers: Option<RxHeaders>,
        /// `(byte offset, phase)` for every cycle a byte was taken.
        phases: Vec<(usize, RxPhase)>,
        icrc: u32,
    }

    /// Drive a frame, optionally ending it early and optionally
    /// re-asserting `start_of_frame` partway through.
    fn drive(frame: &[u8], eof_at: Option<usize>, restart_at: Option<usize>) -> Run {
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
        };
        let eof = eof_at.unwrap_or(frame.len() - 1);
        let mut stream = vec![idle];
        for (k, b) in frame.iter().enumerate().take(eof + 1) {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0 || restart_at == Some(k),
                end_of_frame: k == eof,
            });
        }
        stream.push(idle);
        stream.push(idle);

        let uut = RxParser::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let mut r = Run {
            verdict: RxVerdict::Accept,
            dones: 0,
            payload: Vec::new(),
            headers: None,
            phases: Vec::new(),
            icrc: 0,
        };
        // `with_reset(1)` means output sample `m` reflects input `m-1`,
        // so the byte offset a sample describes is tracked rather than
        // inferred from the cycle index.
        let mut offset = 0usize;
        for (k, s) in uut.run(input).synchronous_sample().enumerate() {
            let o = s.output;
            if k >= 2 && k < eof + 3 {
                r.phases.push((offset, o.phase));
                offset += 1;
            }
            if let Some(b) = o.payload {
                r.payload.push(b.raw() as u8);
            }
            if o.headers.is_some() {
                r.headers = o.headers;
            }
            if o.done {
                r.verdict = o.verdict;
                r.icrc = o.icrc.raw() as u32;
                r.dones += 1;
            }
        }
        r
    }

    fn run(frame: &[u8]) -> Run {
        drive(frame, None, None)
    }

    /// A frame with byte `at` replaced by `to`, and the IPv4 header
    /// checksum left stale on purpose — the parser does not verify it,
    /// and a receiver must not depend on it, because the ICRC is what
    /// actually protects the header.
    fn mutate(frame: &[u8], at: usize, to: u8) -> Vec<u8> {
        let mut f = frame.to_vec();
        assert_ne!(
            f[at], to,
            "mutation at offset {at} would change nothing — it is \
             already {to:#04x}, so this test would pass vacuously"
        );
        f[at] = to;
        f
    }

    // --------------------------------------------- Tier 1: the kernel

    #[test]
    fn payload_length_is_the_total_length_less_the_overhead() {
        // With a RETH: 20 + 8 + 12 + 16 + 4 = 60 bytes of overhead.
        assert_eq!(payload_len_of(bits(60), true).raw(), 0);
        assert_eq!(payload_len_of(bits(76), true).raw(), 16);
        // Without: 44.
        assert_eq!(payload_len_of(bits(44), false).raw(), 0);
        assert_eq!(payload_len_of(bits(60), false).raw(), 16);
        // The largest PMTU.
        assert_eq!(payload_len_of(bits(60 + 4096), true).raw(), 4096);
    }

    /// **`resize` truncates, it does not saturate.** A `total_len`
    /// below the overhead must clamp to zero rather than wrapping to
    /// something enormous, which is what an unclamped subtraction in
    /// 16-bit arithmetic would produce.
    #[test]
    fn payload_length_clamps_rather_than_wrapping() {
        for tl in 0u128..60 {
            let n = payload_len_of(bits(tl), true).raw();
            assert!(
                n == 0 || tl >= 60,
                "total_len {tl} with a RETH implies no payload, got {n}"
            );
        }
        assert_eq!(payload_len_of(bits(0), true).raw(), 0);
        assert_eq!(payload_len_of(bits(43), false).raw(), 0);
    }

    #[test]
    fn payload_len_of_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..300)
            .flat_map(|n| [(bits::<16>(n * 17), true), (bits::<16>(n * 17), false)])
            .chain([(bits::<16>(65535), true), (bits::<16>(0), false)]);
        test_kernel_vm_and_verilog_synchronous::<payload_len_of, _, _, _>(payload_len_of, inputs)?;
        Ok(())
    }

    // ------------------------------- Tier 2: whole-widget simulation

    /// The control for every mutation test below. Without this, a
    /// parser that rejected everything would pass all of them.
    #[test]
    fn a_well_formed_frame_is_accepted() {
        for (op, payload, pad) in [
            (WriteOpcode::Only, 16, 0),
            (WriteOpcode::Only, 0, 0),
            (WriteOpcode::First, 64, 0),
            (WriteOpcode::Middle, 64, 0),
            (WriteOpcode::Last, 13, 3),
            (WriteOpcode::OnlyWithImmediate, 8, 0),
        ] {
            let f = good_frame(op, payload, pad);
            let r = run(&f);
            assert_eq!(r.verdict, RxVerdict::Accept, "{op:?}/{payload}/{pad}");
            assert_eq!(r.dones, 1, "{op:?}: exactly one done");
            assert_eq!(
                r.payload.len(),
                (payload + pad) as usize,
                "{op:?}: payload byte count"
            );
            let h = r.headers.expect("headers");
            assert_eq!(h.kind.op, op, "{op:?}: recovered operation");
            assert_eq!(h.has_reth, carries_reth(op), "{op:?}: RETH presence");
            assert_eq!(r.icrc, 0xDEAD_BEEF, "{op:?}: trailer bytes");
        }
    }

    /// **A packet without a RETH reports a zero RETH, not payload.**
    ///
    /// The header block of a MIDDLE or LAST packet ends at offset 54,
    /// so byte 69 -- where a RETH-carrying packet's RETH ends -- is a
    /// payload byte. An offset-only snapshot would load the RETH
    /// register with payload data. `has_reth` is false there, so a
    /// correct consumer ignores it; this pins the value anyway, because
    /// the failure mode of forgetting that check should be "writes to
    /// address zero", not "writes to an address made of payload bytes".
    ///
    /// Not caught by the corpus tests, which compare the RETH only when
    /// `has_reth` is set. Found by reading the code.
    #[test]
    fn a_packet_without_a_reth_reports_a_zero_reth() {
        for (op, payload) in [
            (WriteOpcode::Middle, 64u128),
            (WriteOpcode::Last, 32),
            (WriteOpcode::LastWithImmediate, 16),
        ] {
            let f = good_frame(op, payload, 0);
            assert!(
                f.len() > 70,
                "{op:?}: the frame must reach past offset 69 for this \
                 test to mean anything -- it is {} bytes",
                f.len()
            );
            let r = run(&f);
            assert_eq!(r.verdict, RxVerdict::Accept, "{op:?}");
            let h = r.headers.expect("headers");
            assert!(!h.has_reth, "{op:?} carries no RETH");
            assert_eq!(
                h.reth,
                Reth::default(),
                "{op:?}: the RETH must be zero, not payload bytes -- \
                 byte 69 of this frame is payload"
            );
        }
    }

    /// **Each rejection, provoked by exactly one byte.** The frame is
    /// well-formed apart from the mutation, and
    /// [`a_well_formed_frame_is_accepted`] proves the unmutated version
    /// passes — so a wrong verdict here is attributable to the one byte
    /// that changed.
    #[test]
    fn each_malformed_field_produces_its_own_verdict() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        for (at, to, want, why) in [
            (12, 0x86, RxVerdict::NotIpv4, "EtherType high byte — IPv6"),
            (13, 0x06, RxVerdict::NotIpv4, "EtherType low byte — ARP-ish"),
            (
                14,
                0x46,
                RxVerdict::Ipv4Unsupported,
                "IHL 6 — IP options present",
            ),
            (
                14,
                0x65,
                RxVerdict::Ipv4Unsupported,
                "version 6 in an IPv4 EtherType frame",
            ),
            (23, 0x06, RxVerdict::NotUdp, "protocol TCP"),
            (23, 0x01, RxVerdict::NotUdp, "protocol ICMP"),
            (
                37,
                0xB6,
                RxVerdict::NotRocePort,
                "UDP dest port 4790 — one below RoCE",
            ),
            (
                36,
                0x11,
                RxVerdict::NotRocePort,
                "UDP dest port high byte wrong",
            ),
            (
                42,
                0x00,
                RxVerdict::UnknownOpcode,
                "RC SEND_FIRST — a SEND, not a WRITE",
            ),
            (
                42,
                0x40,
                RxVerdict::UnknownOpcode,
                "Reliable Datagram transport",
            ),
            (
                42,
                0x2C,
                RxVerdict::UnknownOpcode,
                "UC operation 0x0C — just past WRITE_ONLY_IMM",
            ),
        ] {
            let r = run(&mutate(&good, at, to));
            assert_eq!(
                r.verdict, want,
                "offset {at} set to {to:#04x} ({why}) should give {want:?}"
            );
            assert_eq!(r.dones, 1, "offset {at}: exactly one done");
        }
    }

    /// A bad IP total length is caught at offset 17, whichever
    /// direction it is wrong in.
    #[test]
    fn an_impossible_ip_total_length_is_rejected() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        // Too short to hold even an empty no-RETH packet.
        let mut f = good.clone();
        f[16] = 0x00;
        f[17] = 43;
        assert_eq!(run(&f).verdict, RxVerdict::BadLength, "total_len 43");
        // Zero.
        let mut f = good.clone();
        f[16] = 0;
        f[17] = 0;
        assert_eq!(run(&f).verdict, RxVerdict::BadLength, "total_len 0");
        // Beyond the largest PMTU plus overhead.
        let mut f = good.clone();
        let big = MAX_IP_TOTAL_LEN + 1;
        f[16] = (big >> 8) as u8;
        f[17] = (big & 0xFF) as u8;
        assert_eq!(
            run(&f).verdict,
            RxVerdict::BadLength,
            "total_len {big} exceeds the largest PMTU plus overhead"
        );
        // And the boundary is inclusive on both sides.
        let mut f = good.clone();
        f[16] = 0;
        f[17] = MIN_IP_TOTAL_LEN as u8;
        assert_ne!(
            run(&f).verdict,
            RxVerdict::BadLength,
            "total_len {MIN_IP_TOTAL_LEN} is the legal minimum"
        );
    }

    /// **A field the parser does not check must not be rejected.** The
    /// complement of the test above: without this, a parser that
    /// rejected on any mutation at all would look correct.
    #[test]
    fn mutating_an_unchecked_field_does_not_reject() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        for (at, to, why) in [
            (0, 0xFF, "destination MAC — the MAC filter's business"),
            (6, 0xFF, "source MAC"),
            (15, 0x2E, "DSCP/ECN — masked by the ICRC, so it may change"),
            (18, 0xAB, "IP identification"),
            (22, 0x01, "TTL — decremented by every router in the path"),
            (24, 0x00, "IPv4 header checksum — not verified here"),
            (34, 0xFF, "UDP source port — carries no RoCE meaning"),
            (40, 0xFF, "UDP checksum — masked by the ICRC"),
            (43, 0xFF, "BTH solicited/migreq/pad/version bits"),
            (46, 0xFF, "BTH FECN/BECN/reserved — masked by the ICRC"),
        ] {
            let r = run(&mutate(&good, at, to));
            assert_eq!(
                r.verdict,
                RxVerdict::Accept,
                "offset {at} set to {to:#04x} ({why}) is not a field this \
                 widget validates, so the frame must still be accepted"
            );
        }
    }

    /// A frame that ends early is truncated, wherever it ends — in the
    /// middle of the headers, of the payload, or of the ICRC.
    #[test]
    fn a_frame_that_ends_early_is_truncated() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        assert_eq!(good.len(), 90, "70 header + 16 payload + 4 ICRC");
        for eof in [0, 13, 17, 41, 53, 69, 70, 85, 88] {
            let r = drive(&good, Some(eof), None);
            assert_eq!(
                r.verdict,
                RxVerdict::Truncated,
                "a frame ending at byte {eof} of 90 is truncated"
            );
            assert_eq!(r.dones, 1, "eof at {eof}: exactly one done");
        }
        // And the full length is not truncated — the boundary matters.
        assert_eq!(drive(&good, Some(89), None).verdict, RxVerdict::Accept);
    }

    /// **Truncation is detected by length, not by phase.** A frame cut
    /// exactly at a phase boundary still has the wrong number of bytes,
    /// and that is what must be noticed.
    #[test]
    fn a_frame_cut_at_a_phase_boundary_is_still_truncated() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        // Byte 69 is the last header byte: a frame ending there has a
        // complete header block and nothing else, so a parser checking
        // "did the headers finish?" would wrongly accept it.
        let r = drive(&good, Some(69), None);
        assert_eq!(r.verdict, RxVerdict::Truncated);
        assert!(
            r.payload.is_empty(),
            "and no payload byte should have escaped: {:?}",
            r.payload
        );
    }

    /// Bytes still arriving after the declared length are an overlong
    /// frame, reported rather than silently consumed.
    #[test]
    fn a_frame_longer_than_it_declared_is_rejected() {
        let mut f = good_frame(WriteOpcode::Only, 16, 0);
        f.extend([0x00; 8]);
        let r = run(&f);
        assert_eq!(r.verdict, RxVerdict::Overlong);
        assert_eq!(r.dones, 1);
    }

    /// A byte arriving without a start-of-frame while idle is the tail
    /// of a frame whose beginning was missed. Dropping it is how the
    /// parser realigns, and it must not be mistaken for a new frame.
    #[test]
    fn bytes_without_a_start_of_frame_are_dropped_while_idle() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
        };
        // The same frame, but with no `start_of_frame` anywhere.
        let last = good.len() - 1;
        let mut stream = vec![idle];
        for (k, b) in good.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: false,
                end_of_frame: k == last,
            });
        }
        stream.push(idle);

        let uut = RxParser::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        let mut dones = 0;
        let mut payload = 0;
        let mut ever_busy = false;
        for s in uut.run(input).synchronous_sample() {
            if s.output.done {
                dones += 1;
            }
            if s.output.payload.is_some() {
                payload += 1;
            }
            if s.output.busy {
                ever_busy = true;
            }
        }
        assert_eq!(dones, 0, "no frame began, so none can have ended");
        assert_eq!(payload, 0, "and no payload should be emitted");
        assert!(!ever_busy, "the parser should never have left Idle");
    }

    /// A start-of-frame part way through abandons the frame in
    /// progress — reported as truncated in that same cycle — and parses
    /// the new one from byte zero.
    #[test]
    fn a_restart_abandons_the_frame_in_progress_and_parses_the_next() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        // Byte 40 of the first frame, then the whole of a second.
        let mut f = good[..40].to_vec();
        let restart = f.len();
        f.extend_from_slice(&good);
        let r = drive(&f, None, Some(restart));
        assert_eq!(
            r.dones, 2,
            "two frames ended: the abandoned one and the complete one"
        );
        assert_eq!(
            r.verdict,
            RxVerdict::Accept,
            "the last verdict is the second frame's, which is well-formed"
        );
        assert_eq!(
            r.payload.len(),
            16,
            "only the second frame's payload should appear"
        );
        let h = r.headers.expect("the second frame's headers");
        assert_eq!(h.bth.dqpn.raw(), 0x000111);
    }

    /// **The phase and the byte offset never disagree.**
    ///
    /// The phase is redundant state — it is derivable from the byte
    /// offset and the frame's geometry — and it is kept because it is
    /// the central question when reading a parser and it is what the
    /// generated diagram is drawn from. This is what stops the
    /// redundancy becoming a bug.
    ///
    /// The boundaries, all of which follow from the frame layout:
    /// `Network` covers the Ethernet/IPv4/UDP bytes and ends on the
    /// *first* UDP payload byte (offset 42), because the phase is the
    /// registered one; `Transport` covers the BTH and, when present,
    /// the RETH; `Payload` the RDMA payload; `Trailer` the four ICRC
    /// bytes.
    #[test]
    fn the_phase_always_agrees_with_the_byte_offset() {
        for (op, payload, pad) in [
            (WriteOpcode::Only, 16, 0),
            (WriteOpcode::Only, 0, 0),
            (WriteOpcode::Middle, 64, 0),
            (WriteOpcode::Last, 13, 3),
        ] {
            let f = good_frame(op, payload, pad);
            // Where the transport headers end, as a frame offset.
            let transport_end = if carries_reth(op) { 70 } else { 54 };
            let n = f.len();
            let r = run(&f);
            for (offset, phase) in &r.phases {
                let want = if *offset == 0 {
                    RxPhase::Idle
                } else if *offset <= 42 {
                    RxPhase::Network
                } else if *offset < transport_end {
                    RxPhase::Transport
                } else if *offset <= n - 4 {
                    RxPhase::Payload
                } else if *offset < n {
                    RxPhase::Trailer
                } else {
                    RxPhase::Idle
                };
                // A zero-payload frame has `transport_end == n - 4`, so
                // the Payload branch is correctly never taken for it.
                assert_eq!(
                    *phase, want,
                    "{op:?}/{payload}/{pad}: at byte offset {offset} of \
                     {n} the phase should be {want:?}, not {phase:?}"
                );
            }
        }
    }

    /// Reset returns the parser to idle mid-frame, and the frame is
    /// abandoned rather than resumed.
    #[test]
    fn reset_abandons_a_frame_in_progress() {
        let good = good_frame(WriteOpcode::Only, 16, 0);
        let cr_reset = clock_reset(clock(false), reset(true));
        let q = Q {
            udp: Default::default(),
            state: RxPhase::Payload,
            extras: RxRegs {
                transport_valid: true,
                ..Default::default()
            },
        };
        let i = In {
            data: Some(bits(0x42)),
            start_of_frame: false,
            end_of_frame: false,
        };
        let (o, d) = rx_parse(cr_reset, i, q);
        assert_eq!(d.state, RxPhase::Idle, "reset returns to idle");
        assert_eq!(d.extras, RxRegs::default(), "and clears every register");
        assert_eq!(o.headers, None, "headers are withdrawn");
        assert_eq!(o.payload, None, "and no payload byte escapes");
        assert!(!o.done, "reset is not the end of a frame");
        assert_eq!(o.verdict, RxVerdict::Accept);
        let _ = good;
    }

    // ------------------------------------ Tier 3: emitted Verilog

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = RxParser::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"), "a top module is emitted");
        // The header accumulator is 128 bits wide and must survive to
        // the Verilog as one register rather than being split.
        assert!(
            hdl.contains("127:0") || hdl.contains("[127"),
            "the 128-bit accumulator should appear in the emitted Verilog"
        );
        Ok(())
    }

    // ------------------------------- Tier 4: iverilog round-trip

    fn short_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In)>> {
        let f = good_frame(WriteOpcode::Middle, 8, 0);
        let idle = In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
        };
        let last = f.len() - 1;
        let mut stream = vec![idle];
        for (k, b) in f.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
            });
        }
        stream.push(idle);
        stream.into_iter().with_reset(1).clock_pos_edge(100)
    }

    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut = RxParser::default();
        let tb = uut
            .run(short_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    // ----------------------------------- Tier 5: waveform digest

    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut = RxParser::default();
        let vcd = uut.run(short_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rx_parse");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["3c343586a1d2ea781debb5308bdda2ac2d608e486a64c6421ea2aac3795a8de2"];
        let digest = vcd.dump_to_file(root.join("rx_parse.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    // ------------------------------------------- the FSM diagram

    /// Per CLAUDE.md §12 rule 14: the committed diagram must agree with
    /// what the extractor produces from the kernel as it now stands.
    #[test]
    fn test_fsm_diagram_matches_descriptor() {
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<RxParser>()
            .expect("extracting transitions");
        let fresh = render_fsm_diagram_markdown::<RxParser>(&transitions);
        let committed = include_str!("../../doc/rx_parse_fsm.md");
        assert_eq!(
            fresh.trim(),
            committed.trim(),
            "doc/rx_parse_fsm.md is stale — re-run \
             `cargo run --example rx_parse -p rhdl-roce`"
        );
    }

    /// The phases and the edges between them, pinned by name.
    ///
    /// Eighteen edges on six nodes, out of thirty possible, and the
    /// absences carry as much as the presences:
    ///
    ///   - `Idle` reaches only `Network`. The only byte that leaves
    ///     `Idle` is a start-of-frame, and that byte is offset zero.
    ///   - `Network` reaches neither `Payload` nor `Trailer`: the
    ///     transport headers are always between them.
    ///   - `Trailer` never returns to `Payload`, and `Drain` reaches no
    ///     data phase at all — once past the payload or rejected there
    ///     is no way back within a frame.
    ///   - `Transport -> Trailer` exists, and should: a zero-length
    ///     WRITE has no payload bytes between its RETH and its ICRC.
    ///
    /// Every phase reaches `Network` (a start-of-frame restarts from
    /// anywhere) and `Idle` (end of frame), which is why the count is
    /// as high as it is.
    ///
    /// Every edge here is one the hardware can take; a diagram is only
    /// worth including in the docs if that is true. The ones that are
    /// *absent* carry as much information as the ones present:
    ///
    ///   - `Idle` reaches only `Header`, because the only byte that
    ///     leaves `Idle` is a start-of-frame, and that is offset zero.
    ///   - `Trailer` never returns to `Payload`, and `Drain` reaches
    ///     neither — once past the payload there is no way back into it
    ///     within a frame.
    ///   - every phase reaches `Header`, which is the restart, and
    ///     `Idle`, which is the end of a frame.
    #[test]
    fn the_fsm_has_exactly_the_edges_the_frame_structure_implies() {
        let t = rhdl::core::fsm::extract_widget_transitions_strict::<RxParser>()
            .expect("extracting transitions");
        let name = [
            "Idle",
            "Network",
            "Transport",
            "Payload",
            "Trailer",
            "Drain",
        ];
        let mut edges: Vec<String> = t
            .iter()
            .filter(|x| x.source_index != x.target_index)
            .map(|x| format!("{} -> {}", name[x.source_index], name[x.target_index]))
            .collect();
        edges.sort();
        edges.dedup();
        let expect = expect![[r#"
            Drain -> Idle
            Drain -> Network
            Idle -> Network
            Network -> Drain
            Network -> Idle
            Network -> Transport
            Payload -> Drain
            Payload -> Idle
            Payload -> Network
            Payload -> Trailer
            Trailer -> Drain
            Trailer -> Idle
            Trailer -> Network
            Transport -> Drain
            Transport -> Idle
            Transport -> Network
            Transport -> Payload
            Transport -> Trailer
        "#]];
        expect.assert_eq(&format!("{}\n", edges.join("\n")));
    }
}
