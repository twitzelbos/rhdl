//! Byte-serial receive parser for Ethernet, IPv4 and UDP.
//!
//! The piece that made [`super`] worth creating. Two protocols in
//! `rhdl-roce` — the RDMA receiver and the connection manager — each
//! needed the same forty-two header bytes taken apart, and the second
//! one shipped with the first one's offsets copied and a note saying
//! this widget was the right answer.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+UdpRx+----------------+
            |                        |
  ?u8       |                        |  ?UdpHeader
 +--------->| data           header +--------->
            |                        |
  bool      |                        |  ?u8
 +--------->| sof           payload +--------->
            |                        |
  bool      |                        |  b16
 +--------->| eof     payload_index +--------->
            |                        |
  b16       |                        |  UdpRxVerdict
 +--------->| expect_port   verdict +--------->
            +------------------------+
")]
//!
//! # It passes every byte through, and that is the design
//!
//! The obvious shape for a layered parser is a filter: consume the
//! headers, emit only the payload. This does that *and* forwards every
//! byte with its frame offset, which looks redundant until you try to
//! build on it.
//!
//! A protocol whose integrity check covers the frame from the IP header
//! onward cannot work from the payload alone — RoCEv2's Invariant CRC
//! covers bytes 14 up to the checksum field, masking seven of them *by
//! frame offset*. A filtering parser would force its caller to keep a
//! second copy of the stream and a second counter to recover what this
//! already knows. So `byte` and `frame_index` come out alongside
//! `payload` and `payload_index`, and a caller uses whichever pair its
//! layer is expressed in.
//!
//! # Internals
#![doc = badascii_doc::badascii!("
   byte
     |
     v
  +--+---------------------+
  | acc = acc<<8 | byte    |     snapshot at each field's last offset:
  |       64 bits          +---> dst_mac(5) src_mac(11) ethertype(13)
  +--+---------------------+     ver_ihl(14) total_len(17) proto(23)
     |                           src_ip(29) dst_ip(33)
     |                           src_port(35) dst_port(37) udp_len(39)
     +--> byte + frame_index : every byte, for a frame-covering checksum
     +--> payload + payload_index : offsets 42 and up
")]
//!
//! 64 bits of accumulator, because a MAC address is the widest single
//! field at six bytes. (A parser that also wanted the IPv6 addresses
//! would need 128.)
//!
//! # What it checks, and what it deliberately does not
//!
//! | Byte | Field | Required |
//! |---:|---|---|
//! | 13 | EtherType | `0x0800` |
//! | 14 | Version / IHL | `0x45` |
//! | 17 | IP total length | at least a header's worth |
//! | 23 | IP protocol | `17` |
//! | 37 | UDP dest port | `expect_port` |
//!
//! Each check fires at the earliest byte that can settle it, so a frame
//! addressed elsewhere is abandoned without parsing the rest of it.
//!
//! **IHL must be exactly 5, not at least 5.** Accepting options would
//! mean a variable header length through every downstream datapath, and
//! the protocols this serves forbid them. Rejecting is both safer and
//! smaller.
//!
//! **Neither checksum is verified** — not IPv4's header checksum, not
//! UDP's. The Ethernet frame check sequence has already been verified by
//! the MAC before a frame reaches here, and a protocol that wants more
//! than that has its own integrity check which this widget's
//! `frame_index` output exists to feed. Verifying the IPv4 checksum here
//! would cost a second accumulator to tell a caller something it mostly
//! cannot act on.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/udp_rx.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/udp_rx.md")]
//!
//! And the state machine:
#![doc = include_str!("../../doc/udp_rx_fsm.md")]

use super::ethernet::ETHERTYPE_IPV4;
use super::ipv4::{IP_PROTO_UDP, IPV4_BYTES, IPV4_OFFSET};
use super::udp::{UDP_BYTES, UDP_OFFSET};
use crate::core::dff;
use rhdl::prelude::*;

/// Frame offset at which the UDP payload begins.
pub const PAYLOAD_OFFSET: u128 = UDP_OFFSET + UDP_BYTES;

/// The smallest IP total length that is not self-contradictory: an IPv4
/// header and a UDP header, with no payload.
pub const MIN_IP_TOTAL_LEN: u128 = IPV4_BYTES + UDP_BYTES;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Why a frame was or was not accepted.
///
/// Reasons rather than one `bool`, because on a receive path these are
/// the only diagnostic available: a counter per variant distinguishes
/// "the link is carrying someone else's broadcast traffic" from "a peer
/// is sending us the wrong protocol", which a single `dropped` count
/// conflates.
pub enum UdpRxVerdict {
    #[default]
    /// Every check has passed so far. On `done`, the headers are
    /// well-formed and the datagram is addressed to `expect_port`.
    Accept,
    /// EtherType was not IPv4.
    NotIpv4,
    /// IPv4 version was not 4, or the header carried options.
    Ipv4Unsupported,
    /// IP total length is smaller than the headers it must contain.
    BadLength,
    /// IP protocol was not UDP.
    NotUdp,
    /// The UDP destination port was not `expect_port`.
    PortMismatch,
    /// The frame ended before the headers were complete, or its length
    /// disagreed with the IP total length field.
    Truncated,
    /// Bytes were still arriving after the IP total length said the
    /// frame had ended.
    Overlong,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Everything recovered from a datagram's Ethernet, IPv4 and UDP
/// headers.
pub struct UdpHeader {
    /// Destination MAC, as addressed. Not checked here — a frame
    /// reaching this widget has already been filtered by the MAC, and
    /// an interface with several addresses needs the value rather than
    /// a yes/no.
    pub dst_mac: Bits<48>,
    /// Source MAC. A reply can be addressed from this without anything
    /// about the peer being configured.
    pub src_mac: Bits<48>,
    /// Source IPv4 address.
    pub src_ip: Bits<32>,
    /// Destination IPv4 address, as addressed.
    pub dst_ip: Bits<32>,
    /// Source UDP port. A reply goes back to this, not to
    /// `expect_port`.
    pub src_port: Bits<16>,
    /// Destination UDP port. Equal to `expect_port` whenever the
    /// verdict is [`UdpRxVerdict::Accept`].
    pub dst_port: Bits<16>,
    /// The IPv4 total length field, as received.
    pub ip_total_len: Bits<16>,
    /// The UDP length field, as received: header plus payload.
    pub udp_len: Bits<16>,
    /// Payload bytes implied by `ip_total_len`, which is the figure to
    /// trust — see [`payload_len_of`].
    pub payload_len: Bits<16>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
#[doc(hidden)]
/// The parser's registers, bundled per CLAUDE.md §3.1.
pub struct UdpRxRegs {
    /// Frame offset of the next byte expected.
    pub offset: Bits<16>,
    /// Rolling accumulator. 48 bits is the widest single field (a MAC
    /// address), so 64 is the smallest convenient width that holds one.
    pub acc: Bits<64>,
    /// The header, as recovered so far.
    pub header: UdpHeader,
    /// Sticky: once it leaves `Accept`, no further checks run.
    pub verdict: UdpRxVerdict,
    /// Set once the whole header block has been accumulated.
    pub header_valid: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
/// Where in a datagram the parser is.
pub enum UdpRxPhase {
    #[default]
    /// Between frames. Only a start-of-frame leaves this state, which
    /// is what lets the parser resynchronise mid-stream.
    Idle,
    /// Accumulating the forty-two header bytes and running the checks.
    Header,
    /// Forwarding payload bytes.
    Payload,
    /// The datagram is not ours; its remaining bytes are consumed and
    /// discarded so the parser is aligned for the next one.
    Drain,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = UdpRxPhase, allow_implicit)]
/// Byte-serial Ethernet/IPv4/UDP receive parser. See the
/// [module docs](self).
pub struct UdpRx {
    state: dff::DFF<UdpRxPhase>,
    regs: dff::DFF<UdpRxRegs>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`UdpRx`].
pub struct In {
    /// The next frame byte, or `None` when the link is idle.
    pub data: Option<Bits<8>>,
    /// This byte is the first of a frame. Required to leave
    /// [`UdpRxPhase::Idle`], and mid-frame it abandons the frame in
    /// progress and starts a new one.
    pub start_of_frame: bool,
    /// This byte is the last of a frame.
    pub end_of_frame: bool,
    /// The UDP destination port to accept. An input rather than a
    /// parameter so one instance can be repointed, and so a caller's
    /// port is its own business.
    pub expect_port: Bits<16>,
}

// `Default` is derived here, unlike most widget `Out` types, because a
// composing widget's kernel unit tests have to build a `Q` containing
// one -- and the default is meaningful rather than arbitrary: idle, no
// byte, nothing accepted or rejected yet.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Outputs from [`UdpRx`].
pub struct Out {
    /// The recovered headers, presented from the cycle the first
    /// payload byte arrives and held to the end of the frame.
    ///
    /// That is the earliest it could be: the last header byte only
    /// reaches the registers at the end of the cycle before.
    pub header: Option<UdpHeader>,
    /// A payload byte, in the cycle it arrives.
    pub payload: Option<Bits<8>>,
    /// `payload`'s offset within the UDP payload, counting from zero.
    pub payload_index: Bits<16>,
    /// This cycle's byte, whatever layer it belongs to, or `None`.
    /// Forwarded for a protocol whose integrity check covers the whole
    /// frame — see the module docs.
    pub byte: Option<Bits<8>>,
    /// `byte`'s absolute offset in the Ethernet frame.
    pub frame_index: Bits<16>,
    /// A frame began this cycle — the cycle to re-seed a checksum on.
    pub frame_start: bool,
    /// The IPv4 total length, from the cycle after offset 17 onward;
    /// zero before that.
    ///
    /// Exposed on its own as well as inside `header`, because `header`
    /// is only presented once the whole header block is parsed — frame
    /// offset 42 — while a protocol whose integrity check starts at
    /// offset 14 needs to know where the frame ends from offset 18.
    /// Without this, such a caller would have to snapshot the length
    /// field itself, which is the duplication this widget exists to
    /// remove.
    pub ip_total_len: Bits<16>,
    /// The frame ended this cycle; `verdict` is final.
    pub done: bool,
    /// The running verdict.
    pub verdict: UdpRxVerdict,
    /// A frame is in progress.
    pub busy: bool,
    /// The phase, for debug and waveform reading.
    pub phase: UdpRxPhase,
}

impl SynchronousIO for UdpRx {
    type I = In;
    type O = Out;
    type Kernel = udp_rx;
}

#[kernel]
/// Payload bytes implied by an IPv4 total length.
///
/// Derived from the IP total length rather than taken from the UDP
/// length field, because the two can disagree on a malformed datagram
/// and the IP length is the one the frame's own extent is checked
/// against. Clamped at the subtraction, since a total length below the
/// headers it must contain would otherwise wrap.
pub fn payload_len_of(ip_total_len: Bits<16>) -> Bits<16> {
    let overhead = bits::<16>(MIN_IP_TOTAL_LEN);
    if ip_total_len > overhead {
        ip_total_len - overhead
    } else {
        bits::<16>(0)
    }
}

#[kernel]
// `if let` is not in the kernel-accepted subset -- `#[kernel]` lowers
// `match` on an `Option`, not sugar over it -- so the single-arm
// matches are the only spelling that compiles to hardware.
#[allow(clippy::single_match)]
/// Kernel for [`UdpRx`].
pub fn udp_rx(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();
    d.state = q.state;
    let mut r = q.regs;

    o.header = None;
    o.payload = None;
    o.payload_index = bits(0);
    o.byte = None;
    o.frame_index = bits(0);
    o.frame_start = false;
    o.ip_total_len = q.regs.header.ip_total_len;
    o.done = false;
    o.verdict = q.regs.verdict;
    o.busy = q.state != UdpRxPhase::Idle;
    o.phase = q.state;

    let byte = match i.data {
        Some(b) => b,
        None => bits(0),
    };
    let valid = match i.data {
        Some(_b) => true,
        None => false,
    };
    // A byte without a start-of-frame while idle is the tail of a frame
    // whose beginning was missed. Dropping it is how the parser
    // realigns.
    let taken = valid && (i.start_of_frame || q.state != UdpRxPhase::Idle);

    // The header, once complete, is presented for the rest of the
    // frame. Suppressed on a restart, because it describes the frame
    // being abandoned rather than the one beginning.
    if q.regs.header_valid && !(taken && i.start_of_frame) {
        o.header = Some(q.regs.header);
    }

    if taken {
        if i.start_of_frame {
            // A restart abandons whatever was in progress, and the
            // abandoned frame is reported in this same cycle -- it did
            // end, and a consumer counting frames must see it. Without
            // this a peer that retries mid-frame makes one frame
            // disappear from every downstream count.
            if q.state != UdpRxPhase::Idle {
                o.done = true;
                o.verdict = UdpRxVerdict::Truncated;
            }
            r = UdpRxRegs::default();
            o.frame_start = true;
        }
        let cur = r.offset;
        let accepting = r.verdict == UdpRxVerdict::Accept;
        let acc = (r.acc << 8) | byte.resize::<64>();

        // Every byte is forwarded with its frame offset, for a
        // protocol whose checksum covers more than the payload.
        o.byte = Some(byte);
        o.frame_index = cur;
        o.ip_total_len = r.header.ip_total_len;

        // Payload passes through in the cycle it arrives, which is the
        // cycle the header is first presented. Both read the
        // *registered* state, so the two stay in step.
        if accepting
            && q.regs.header_valid
            && !i.start_of_frame
            && cur <= r.header.ip_total_len + bits(13)
        {
            o.payload = Some(byte);
            o.payload_index = cur - bits(PAYLOAD_OFFSET);
        }

        if accepting {
            r.offset = cur + bits(1);
            r.acc = acc;

            if cur == bits(5) {
                r.header.dst_mac = (acc & bits(0xFFFF_FFFF_FFFF)).resize::<48>();
            }
            if cur == bits(11) {
                r.header.src_mac = (acc & bits(0xFFFF_FFFF_FFFF)).resize::<48>();
            }
            if cur == bits(13) && (acc & bits(0xFFFF)) != bits(ETHERTYPE_IPV4) {
                r.verdict = UdpRxVerdict::NotIpv4;
            }
            if cur == bits(IPV4_OFFSET) && (acc & bits(0xFF)) != bits(0x45) {
                r.verdict = UdpRxVerdict::Ipv4Unsupported;
            }
            if cur == bits(17) {
                let tl = (acc & bits(0xFFFF)).resize::<16>();
                r.header.ip_total_len = tl;
                r.header.payload_len = payload_len_of(tl);
                if tl < bits(MIN_IP_TOTAL_LEN) {
                    r.verdict = UdpRxVerdict::BadLength;
                }
            }
            if cur == bits(23) && (acc & bits(0xFF)) != bits(IP_PROTO_UDP) {
                r.verdict = UdpRxVerdict::NotUdp;
            }
            if cur == bits(29) {
                r.header.src_ip = (acc & bits(0xFFFF_FFFF)).resize::<32>();
            }
            if cur == bits(33) {
                r.header.dst_ip = (acc & bits(0xFFFF_FFFF)).resize::<32>();
            }
            if cur == bits(35) {
                r.header.src_port = (acc & bits(0xFFFF)).resize::<16>();
            }
            if cur == bits(37) {
                let dp = (acc & bits(0xFFFF)).resize::<16>();
                r.header.dst_port = dp;
                if dp != i.expect_port {
                    r.verdict = UdpRxVerdict::PortMismatch;
                }
            }
            if cur == bits(39) {
                r.header.udp_len = (acc & bits(0xFFFF)).resize::<16>();
            }
            // Only checkable once the length field has been seen;
            // before that the register reads zero and every offset
            // would look long.
            if cur > bits(17) && cur > r.header.ip_total_len + bits(13) {
                r.verdict = UdpRxVerdict::Overlong;
            }
        }

        let after = cur + bits(1);
        if accepting && after >= bits(PAYLOAD_OFFSET) {
            r.header_valid = true;
        }

        // A match on the phase rather than one chain over offsets: the
        // FSM extractor cannot narrow a target that does not depend on
        // `q.state`, and a plain chain makes it report edges the
        // hardware cannot take -- `Idle -> Payload` among them, since
        // the only byte that leaves `Idle` is offset zero.
        let rejected = r.verdict != UdpRxVerdict::Accept;
        match q.state {
            UdpRxPhase::Idle => d.state = UdpRxPhase::Header,
            UdpRxPhase::Header => {
                if rejected {
                    d.state = UdpRxPhase::Drain;
                } else if after < bits(PAYLOAD_OFFSET) {
                    d.state = UdpRxPhase::Header;
                } else {
                    d.state = UdpRxPhase::Payload;
                }
            }
            UdpRxPhase::Payload => {
                if rejected {
                    d.state = UdpRxPhase::Drain;
                } else {
                    d.state = UdpRxPhase::Payload;
                }
            }
            UdpRxPhase::Drain => d.state = UdpRxPhase::Drain,
        }
        // A start-of-frame begins a new header block from whatever
        // phase the parser was in.
        if i.start_of_frame {
            d.state = UdpRxPhase::Header;
        }

        if i.end_of_frame {
            o.done = true;
            // The frame's own extent must agree with what IPv4 claimed,
            // *and* the header block must have completed -- or there
            // was no claim to agree with. Both halves are load-bearing:
            // without the second, a 14-byte frame is accepted, because
            // `ip_total_len` is still zero before offset 17 and
            // `0 + 14` equals the byte count exactly.
            if r.verdict == UdpRxVerdict::Accept {
                if r.header_valid && after == r.header.ip_total_len + bits(14) {
                    o.verdict = UdpRxVerdict::Accept;
                } else {
                    o.verdict = UdpRxVerdict::Truncated;
                }
            } else {
                o.verdict = r.verdict;
            }
            d.state = UdpRxPhase::Idle;
            r = UdpRxRegs::default();
        } else {
            o.verdict = r.verdict;
        }
    }

    if cr.reset.any() {
        d.state = UdpRxPhase::Idle;
        r = UdpRxRegs::default();
        o.header = None;
        o.payload = None;
        o.payload_index = bits(0);
        o.byte = None;
        o.frame_index = bits(0);
        o.frame_start = false;
        o.ip_total_len = bits(0);
        o.done = false;
        o.verdict = UdpRxVerdict::Accept;
        o.busy = false;
        o.phase = UdpRxPhase::Idle;
    }
    d.regs = r;
    (o, d)
}
