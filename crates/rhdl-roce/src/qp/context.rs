//! Per-connection queue-pair parameters.
//!
//! Everything a send queue needs to know about one RDMA connection
//! that does not change from packet to packet. On a real endpoint these
//! are written once by the out-of-band connection manager — the host
//! creates the queue pair, registers its memory region, and sends the
//! resulting numbers over UDP, exactly as FERoCE does — and then held
//! in a register file indexed by queue-pair number.
//!
//! The *message* parameters (where to write, how much) are separate and
//! live in [`MessageRequest`], because they change per message while
//! these do not.

use crate::headers::Transport;
use rhdl::prelude::*;

/// Path MTU — the maximum payload carried by one RoCEv2 packet.
///
/// The IBTA defines exactly these five values and a queue pair
/// negotiates one at connection setup. Making it an enum rather than a
/// number means an un-negotiable MTU cannot be configured, and it keeps
/// the payload-length arithmetic to a shift.
///
/// # Which one is reachable
///
/// The MTU has to fit inside the link's frame budget along with 58
/// bytes of Ethernet/IP/UDP/BTH header, a 16-byte RETH and the 4-byte
/// ICRC. The Zynq-7000 PS GEM caps frames at 1522 bytes, which makes
/// [`Pmtu::B1024`] the largest reachable value on that path — see
/// `roce-v2-architecture.md` §4.3. Jumbo frames would reach
/// [`Pmtu::B4096`], but they are a PL-side AXI Ethernet feature and the
/// PS GEM does not have them.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum Pmtu {
    /// 256 bytes.
    B256,
    /// 512 bytes.
    B512,
    /// 1024 bytes — the largest that fits the PS GEM's 1522-byte frame.
    #[default]
    B1024,
    /// 2048 bytes. Needs jumbo frames.
    B2048,
    /// 4096 bytes. Needs jumbo frames.
    B4096,
}

/// Width in bits of a payload-byte count. 13 bits holds 4096.
pub const PAYLOAD_LEN_BITS: usize = 13;

#[kernel]
/// The path MTU as a byte count.
pub fn pmtu_bytes(m: Pmtu) -> Bits<13> {
    match m {
        Pmtu::B256 => bits(256),
        Pmtu::B512 => bits(512),
        Pmtu::B1024 => bits(1024),
        Pmtu::B2048 => bits(2048),
        Pmtu::B4096 => bits(4096),
    }
}

/// The parameters of one RDMA connection.
///
/// Supplied by the out-of-band connection manager; constant for the
/// life of the queue pair.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct QpContext {
    /// Destination queue pair number, as the responder created it.
    pub dqpn: Bits<24>,
    /// Transport service. [`Transport::Uc`] is what
    /// `roce-v2-architecture.md` §5.3 targets first.
    pub transport: Transport,
    /// Negotiated path MTU.
    pub pmtu: Pmtu,
    /// Remote key authorising access to the responder's memory region.
    pub rkey: Bits<32>,
    /// Partition key. `0xFFFF` on a single-partition fabric.
    pub pkey: Bits<16>,
}

/// One RDMA WRITE message to send.
///
/// A message is what the *application* asks for; the segmenter turns it
/// into however many packets the path MTU requires.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MessageRequest {
    /// Virtual address in the responder's address space to write the
    /// first byte at. With GPUDirect RDMA this is a GPU address.
    pub va: Bits<64>,
    /// Total length of the message in bytes. May be zero, which is a
    /// legal single-packet RDMA WRITE that moves no data.
    pub length: Bits<32>,
}
