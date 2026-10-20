//! The InfiniBand transport headers that RoCEv2 carries over UDP.
//!
//! Two records, both laid out exactly as they appear on the wire:
//! [`Bth`], the 12-byte Base Transport Header present on every packet,
//! and [`Reth`], the 16-byte RDMA Extended Transport Header present on
//! the first or only packet of an RDMA WRITE.
//!
//! # Opcodes are a type, not a byte
//!
//! The IBTA opcode is a transport service in the top three bits and an
//! operation in the low five, and the legal combinations are not the
//! cross product: Unreliable Connection supports SEND and RDMA WRITE
//! and nothing else. [`Transport`] and [`WriteOpcode`] make the
//! combinations this crate implements the only ones that can be named —
//! there is no `WriteOpcode::ReadRequest` to pass by mistake, and
//! [`encode_write_opcode`] is the only way to produce the byte.
//!
//! The [`Bth`] record itself still carries a raw `Bits<8>` opcode,
//! because it is a *wire format* record and has to be able to hold
//! whatever arrived — including an opcode we do not implement. Build it
//! through [`encode_write_opcode`] and that distinction costs nothing.
//!
//! # Serialisation
//!
//! Each header has a `*_pack` kernel that produces the whole header as
//! one wide [`Bits`], and a `*_byte` kernel that selects one byte of it
//! by index, counting from the first byte on the wire. Packing once and
//! indexing is cheaper in hardware than a 12-way or 16-way mux of
//! individually-assembled bytes, and it makes the field offsets
//! auditable in a single place.
//!
//! Both `*_byte` kernels **clamp** their index. An out-of-range index
//! returns the last byte rather than wrapping, because the natural
//! expression — shift by `8 * (len - 1 - index)` — underflows on an
//! unsigned index and produces a shift beyond the operand width, which
//! the kernel VM rejects outright. See CLAUDE.md §4: in a mux both arms
//! evaluate, so the operand has to be clamped and not merely the
//! result.

use rhdl::prelude::*;

/// Transport service class — the top three bits of the opcode.
///
/// Only the two connected services are listed. Reliable and Unreliable
/// Datagram exist in the IBTA opcode space but carry different header
/// stacks and are not implemented here.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum Transport {
    /// Reliable Connection — acknowledged, retransmitted, ordered.
    #[default]
    Rc,
    /// Unreliable Connection — no acknowledgement and no retransmission.
    ///
    /// A packet sequence number gap costs the remainder of one message
    /// and resynchronises on the next FIRST or ONLY, rather than tearing
    /// down the connection. That failure mode is why
    /// `roce-v2-architecture.md` §5.3 targets this transport first for
    /// streaming sensor data.
    Uc,
}

/// The RDMA WRITE family of operations — the low five bits of the
/// opcode.
///
/// A message that fits in one path-MTU packet is sent as [`Only`]. A
/// longer one is [`First`], zero or more [`Middle`]s, then [`Last`].
/// Only `First` and `Only` carry a [`Reth`]; the receiver tracks the
/// running address for the rest.
///
/// [`Only`]: WriteOpcode::Only
/// [`First`]: WriteOpcode::First
/// [`Middle`]: WriteOpcode::Middle
/// [`Last`]: WriteOpcode::Last
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum WriteOpcode {
    /// First packet of a multi-packet message. Carries a [`Reth`].
    First,
    /// A middle packet. No [`Reth`].
    Middle,
    /// Final packet of a multi-packet message. No [`Reth`].
    Last,
    /// Final packet, with 4 bytes of immediate data appended.
    LastWithImmediate,
    /// The whole message in one packet. Carries a [`Reth`].
    #[default]
    Only,
    /// The whole message in one packet, with immediate data.
    OnlyWithImmediate,
}

/// Does this operation carry an [`Reth`]?
///
/// Exposed because the header length — and therefore every downstream
/// byte offset — depends on it.
#[kernel]
pub fn carries_reth(op: WriteOpcode) -> bool {
    match op {
        WriteOpcode::First | WriteOpcode::Only | WriteOpcode::OnlyWithImmediate => true,
        WriteOpcode::Middle | WriteOpcode::Last | WriteOpcode::LastWithImmediate => false,
    }
}

#[kernel]
/// The transport base value, i.e. the opcode's top three bits in place.
pub fn transport_base(t: Transport) -> Bits<8> {
    match t {
        Transport::Rc => bits(0x00),
        Transport::Uc => bits(0x20),
    }
}

#[kernel]
/// The operation offset within a transport's opcode block.
pub fn write_operation(op: WriteOpcode) -> Bits<8> {
    match op {
        WriteOpcode::First => bits(0x06),
        WriteOpcode::Middle => bits(0x07),
        WriteOpcode::Last => bits(0x08),
        WriteOpcode::LastWithImmediate => bits(0x09),
        WriteOpcode::Only => bits(0x0A),
        WriteOpcode::OnlyWithImmediate => bits(0x0B),
    }
}

#[kernel]
/// Encode a transport and an RDMA WRITE operation into the BTH opcode
/// byte.
///
/// This is the only way to produce an opcode in this crate, which is
/// what makes the illegal combinations unreachable rather than merely
/// discouraged. It is a kernel, so the send FSM can select the opcode
/// per packet in hardware and a host-side testbench can call it
/// directly.
pub fn encode_write_opcode(t: Transport, op: WriteOpcode) -> Bits<8> {
    transport_base(t) | write_operation(op)
}

/// Base Transport Header — 12 bytes, present on every RoCEv2 packet.
///
/// Field order and widths are the wire layout. The three reserved
/// groups are carried explicitly rather than being implied, because
/// [`crate::icrc`] has to mask one of them and a field that does not
/// exist cannot be masked.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Bth {
    /// Transport service and operation. Build with
    /// [`encode_write_opcode`].
    pub opcode: Bits<8>,
    /// Solicited event — asks the responder to signal its consumer.
    pub solicited: bool,
    /// Migration request. Unused here; zero.
    pub migreq: bool,
    /// Number of pad bytes, 0–3, appended to bring the payload to a
    /// 4-byte boundary. The ragged-tail mechanism.
    pub padcount: Bits<2>,
    /// Transport header version. Zero.
    pub version: Bits<4>,
    /// Partition key. `0xFFFF` on a single-partition fabric.
    pub pkey: Bits<16>,
    /// Forward ECN. **Masked by the ICRC** — a switch may set it.
    pub fecn: bool,
    /// Backward ECN. **Masked by the ICRC.**
    pub becn: bool,
    /// Reserved, 6 bits. **Masked by the ICRC**, together with the two
    /// ECN bits above, so the whole byte reads `0xFF` to the CRC.
    pub resv6: Bits<6>,
    /// Destination queue pair number.
    pub dqpn: Bits<24>,
    /// The requester wants this packet acknowledged. Meaningless on
    /// [`Transport::Uc`], which never acknowledges.
    pub ackreq: bool,
    /// Reserved, 7 bits. Not masked.
    pub resv7: Bits<7>,
    /// Packet sequence number.
    pub psn: Bits<24>,
}

/// The BTH's length on the wire, in bytes.
pub const BTH_BYTES: usize = 12;

/// RDMA Extended Transport Header — 16 bytes, on the first or only
/// packet of an RDMA WRITE and on an RDMA READ request.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Reth {
    /// Virtual address in the *responder's* address space. With
    /// GPUDirect RDMA this is a GPU address.
    pub va: Bits<64>,
    /// Remote key authorising access to that memory region.
    pub rkey: Bits<32>,
    /// Length in bytes of the whole message, not of this packet.
    pub dma_len: Bits<32>,
}

/// The RETH's length on the wire, in bytes.
pub const RETH_BYTES: usize = 16;

/// Widen a `bool` to a `Bits<N>` of 1 or 0.
///
/// RHDL has no implicit bool-to-bits coercion, and spelling the mux out
/// once here keeps the packing kernels readable.
#[kernel]
pub fn bit_of<const N: usize>(b: bool) -> Bits<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    if b { bits(1) } else { bits(0) }
}

#[kernel]
/// Pack a [`Bth`] into its 96 wire bits, first wire byte in the most
/// significant position.
pub fn bth_pack(h: Bth) -> Bits<96> {
    // byte 0      : opcode
    // byte 1      : solicited | migreq | padcount(2) | version(4)
    // bytes 2..3  : pkey
    // byte 4      : fecn | becn | resv6(6)
    // bytes 5..7  : dqpn(24)
    // byte 8      : ackreq | resv7(7)
    // bytes 9..11 : psn(24)
    (h.opcode.resize::<96>() << 88)
        | (bit_of::<96>(h.solicited) << 87)
        | (bit_of::<96>(h.migreq) << 86)
        | (h.padcount.resize::<96>() << 84)
        | (h.version.resize::<96>() << 80)
        | (h.pkey.resize::<96>() << 64)
        | (bit_of::<96>(h.fecn) << 63)
        | (bit_of::<96>(h.becn) << 62)
        | (h.resv6.resize::<96>() << 56)
        | (h.dqpn.resize::<96>() << 32)
        | (bit_of::<96>(h.ackreq) << 31)
        | (h.resv7.resize::<96>() << 24)
        | h.psn.resize::<96>()
}

#[kernel]
/// Select one byte of a packed [`Bth`], counting from the first byte on
/// the wire.
///
/// `index` is clamped to the last byte; see the module docs for why the
/// clamp is on the operand rather than the result.
pub fn bth_byte(h: Bth, index: Bits<4>) -> Bits<8> {
    let packed = bth_pack(h);
    let safe = if index > bits(11) {
        bits::<4>(11)
    } else {
        index
    };
    let shift = (bits::<8>(11) - safe.resize::<8>()) << 3;
    ((packed >> shift.resize::<96>()) & bits(0xFF)).resize::<8>()
}

#[kernel]
/// Pack a [`Reth`] into its 128 wire bits, first wire byte most
/// significant.
pub fn reth_pack(h: Reth) -> Bits<128> {
    (h.va.resize::<128>() << 64) | (h.rkey.resize::<128>() << 32) | h.dma_len.resize::<128>()
}

#[kernel]
/// Select one byte of a packed [`Reth`], counting from the first byte
/// on the wire. `index` is clamped to the last byte.
pub fn reth_byte(h: Reth, index: Bits<5>) -> Bits<8> {
    let packed = reth_pack(h);
    let safe = if index > bits(15) {
        bits::<5>(15)
    } else {
        index
    };
    let shift = (bits::<8>(15) - safe.resize::<8>()) << 3;
    ((packed >> shift.resize::<128>()) & bits(0xFF)).resize::<8>()
}
