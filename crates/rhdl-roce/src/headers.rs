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

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A decoded RDMA WRITE opcode: which transport service, and which
/// operation within it.
///
/// A struct rather than a tuple because a kernel matching
/// `Some((t, op))` is a nested pattern, which the kernel language
/// rejects; `Some(kind)` binds a whole struct and is accepted. See
/// `kernel-language-extensions.md` §4.6.
pub struct WriteKind {
    /// The transport service, from the opcode's top three bits.
    pub transport: Transport,
    /// The operation, from the low five bits.
    pub op: WriteOpcode,
}

#[kernel]
/// Decode an opcode byte — the inverse of [`encode_write_opcode`].
///
/// `None` for any byte that is not an RDMA WRITE opcode of a transport
/// service this crate implements, which on the receive path is a
/// rejection rather than an error: the frame was addressed to us but
/// asks for something we do not do.
///
/// Returning `Option` rather than saturating to a default is the point.
/// An unrecognised opcode must not be silently read as `Only`, because
/// `Only` carries a RETH and would make the parser consume 16 bytes of
/// payload as an address.
pub fn decode_write_opcode(b: Bits<8>) -> Option<WriteKind> {
    let base = b & bits(0xE0);
    let operation = b & bits(0x1F);
    let mut kind = WriteKind::dont_care();
    // Both halves must be recognised; `ok` is cleared by either.
    let mut ok = true;
    if base == bits(0x00) {
        kind.transport = Transport::Rc;
    } else if base == bits(0x20) {
        kind.transport = Transport::Uc;
    } else {
        kind.transport = Transport::Rc;
        ok = false;
    }
    if operation == bits(0x06) {
        kind.op = WriteOpcode::First;
    } else if operation == bits(0x07) {
        kind.op = WriteOpcode::Middle;
    } else if operation == bits(0x08) {
        kind.op = WriteOpcode::Last;
    } else if operation == bits(0x09) {
        kind.op = WriteOpcode::LastWithImmediate;
    } else if operation == bits(0x0A) {
        kind.op = WriteOpcode::Only;
    } else if operation == bits(0x0B) {
        kind.op = WriteOpcode::OnlyWithImmediate;
    } else {
        kind.op = WriteOpcode::Only;
        ok = false;
    }
    if ok { Some(kind) } else { None }
}

#[kernel]
/// Unpack a [`Bth`] from its 96 wire bits — the inverse of
/// [`bth_pack`].
///
/// The receive direction's entry point. Keeping it adjacent to
/// `bth_pack` is deliberate: the two share one field-offset layout, and
/// a layout change that updates only one of them is the defect this
/// arrangement is meant to make obvious. `bth_round_trips` pins them
/// against each other.
pub fn bth_unpack(p: Bits<96>) -> Bth {
    let mut h = Bth::dont_care();
    h.opcode = ((p >> 88) & bits(0xFF)).resize::<8>();
    h.solicited = ((p >> 87) & bits(1)) != bits(0);
    h.migreq = ((p >> 86) & bits(1)) != bits(0);
    h.padcount = ((p >> 84) & bits(3)).resize::<2>();
    h.version = ((p >> 80) & bits(0xF)).resize::<4>();
    h.pkey = ((p >> 64) & bits(0xFFFF)).resize::<16>();
    h.fecn = ((p >> 63) & bits(1)) != bits(0);
    h.becn = ((p >> 62) & bits(1)) != bits(0);
    h.resv6 = ((p >> 56) & bits(0x3F)).resize::<6>();
    h.dqpn = ((p >> 32) & bits(0xFF_FFFF)).resize::<24>();
    h.ackreq = ((p >> 31) & bits(1)) != bits(0);
    h.resv7 = ((p >> 24) & bits(0x7F)).resize::<7>();
    h.psn = (p & bits(0xFF_FFFF)).resize::<24>();
    h
}

#[kernel]
/// Unpack a [`Reth`] from its 128 wire bits — the inverse of
/// [`reth_pack`].
pub fn reth_unpack(p: Bits<128>) -> Reth {
    let mut h = Reth::dont_care();
    h.va = ((p >> 64) & bits(0xFFFF_FFFF_FFFF_FFFF)).resize::<64>();
    h.rkey = ((p >> 32) & bits(0xFFFF_FFFF)).resize::<32>();
    h.dma_len = (p & bits(0xFFFF_FFFF)).resize::<32>();
    h
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

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;

    /// A spread of BTH field values, including the all-ones and
    /// all-zeros corners of every field.
    fn bths() -> impl Iterator<Item = Bth> + Clone {
        (0u128..64).map(|i| Bth {
            opcode: bits((i * 4) & 0xFF),
            solicited: i & 1 != 0,
            migreq: i & 2 != 0,
            padcount: bits(i & 3),
            version: bits(i & 0xF),
            pkey: bits(if i == 0 {
                0
            } else {
                0xFFFF ^ ((i * 977) & 0xFFFF)
            }),
            fecn: i & 4 != 0,
            becn: i & 8 != 0,
            resv6: bits(i & 0x3F),
            dqpn: bits((i * 262_111) & 0xFF_FFFF),
            ackreq: i & 16 != 0,
            resv7: bits(i & 0x7F),
            psn: bits((i * 199_933) & 0xFF_FFFF),
        })
    }

    fn reths() -> impl Iterator<Item = Reth> + Clone {
        (0u128..48).map(|i| Reth {
            va: bits((i * 0x0123_4567_89AB_CDEF) & u64::MAX as u128),
            rkey: bits((i * 0x0101_0101) & 0xFFFF_FFFF),
            dma_len: bits((i * 7919) & 0xFFFF_FFFF),
        })
    }

    const OPS: [WriteOpcode; 6] = [
        WriteOpcode::First,
        WriteOpcode::Middle,
        WriteOpcode::Last,
        WriteOpcode::LastWithImmediate,
        WriteOpcode::Only,
        WriteOpcode::OnlyWithImmediate,
    ];

    // Every kernel in this module gets a kernel-VM and `iverilog`
    // round-trip. Per CLAUDE.md §4 a direct Rust call is *more
    // permissive* than the VM -- `Bits<N> << k` for `k >= N` wraps in
    // Rust and errors in the VM -- so the shift-heavy byte selectors
    // here are exactly the kind of kernel that can pass a Tier-1 test
    // and still be broken hardware.

    /// **Pack and unpack are inverses.** The two share one field-offset
    /// layout, and this is what stops a layout change updating only one
    /// of them — which would be invisible on the transmit side and
    /// corrupt every received header.
    /// **Exhaustive over the whole input space.** Every one of the 256
    /// opcode bytes is either rejected or decodes to a kind that
    /// re-encodes to itself, and every constructible kind decodes back
    /// to itself. There is no sampling here and so nothing to miss.
    #[test]
    fn opcode_decoding_is_exactly_the_inverse_of_encoding() {
        let transports = [Transport::Rc, Transport::Uc];
        let ops = [
            WriteOpcode::First,
            WriteOpcode::Middle,
            WriteOpcode::Last,
            WriteOpcode::LastWithImmediate,
            WriteOpcode::Only,
            WriteOpcode::OnlyWithImmediate,
        ];
        // Forward: every legal pair survives the round trip.
        let mut legal = std::collections::BTreeSet::new();
        for t in transports {
            for op in ops {
                let byte = encode_write_opcode(t, op);
                legal.insert(byte.raw());
                assert_eq!(
                    decode_write_opcode(byte),
                    Some(WriteKind { transport: t, op }),
                    "{t:?}/{op:?} encoded to {byte:?} and did not decode back"
                );
            }
        }
        assert_eq!(legal.len(), 12, "two transports times six operations");

        // Backward: acceptance is exactly that set, and an accepted
        // byte re-encodes to itself.
        for raw in 0u128..256 {
            let b = bits::<8>(raw);
            match decode_write_opcode(b) {
                Some(k) => {
                    assert!(
                        legal.contains(&raw),
                        "{raw:#04x} decoded to {k:?} but is not a legal WRITE opcode"
                    );
                    assert_eq!(
                        encode_write_opcode(k.transport, k.op),
                        b,
                        "{raw:#04x} decoded to {k:?} which re-encodes differently"
                    );
                }
                None => assert!(
                    !legal.contains(&raw),
                    "{raw:#04x} is a legal WRITE opcode but was rejected"
                ),
            }
        }
    }

    /// The rejections that matter, named individually so a regression
    /// says which one broke.
    #[test]
    fn opcode_decoding_rejects_the_neighbours_of_the_legal_set() {
        for (raw, why) in [
            (0x0Au128, "RC WRITE_ONLY is legal"),
            (0x2A, "UC WRITE_ONLY is legal"),
        ] {
            assert!(decode_write_opcode(bits::<8>(raw)).is_some(), "{why}");
        }
        for (raw, why) in [
            (0x00u128, "RC SEND_FIRST — a SEND, not a WRITE"),
            (0x05, "RC immediately below WRITE_FIRST"),
            (0x0C, "RC immediately above WRITE_ONLY_IMM"),
            (0x40, "Reliable Datagram — a transport we do not implement"),
            (0x60, "Unreliable Datagram"),
            (0x80, "an unassigned transport"),
            (0xFF, "all ones"),
            (
                0x1A,
                "WRITE_ONLY operation bits, but an RC base of 0x00                     requires the top three bits clear — 0x1A has bit 4 set,                     which belongs to the operation field, so this is                     operation 0x1A and not a WRITE at all",
            ),
        ] {
            assert_eq!(
                decode_write_opcode(bits::<8>(raw)),
                None,
                "{raw:#04x} should be rejected: {why}"
            );
        }
    }

    #[test]
    fn decode_write_opcode_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..256).map(|raw| (bits::<8>(raw),));
        test_kernel_vm_and_verilog_synchronous::<decode_write_opcode, _, _, _>(
            decode_write_opcode,
            inputs,
        )?;
        Ok(())
    }

    #[test]
    fn bth_round_trips_through_pack_and_unpack() {
        for h in bths() {
            let back = bth_unpack(bth_pack(h));
            assert_eq!(back, h, "BTH did not survive pack/unpack: {h:?}");
        }
    }

    /// And the sharper version: every field exercised at its extremes,
    /// so a mask that is one bit too narrow or an offset one bit out
    /// cannot hide behind the spread above.
    #[test]
    fn bth_round_trips_with_every_field_at_its_limits() {
        let all_ones = Bth {
            opcode: bits(0xFF),
            solicited: true,
            migreq: true,
            padcount: bits(3),
            version: bits(0xF),
            pkey: bits(0xFFFF),
            fecn: true,
            becn: true,
            resv6: bits(0x3F),
            dqpn: bits(0xFF_FFFF),
            ackreq: true,
            resv7: bits(0x7F),
            psn: bits(0xFF_FFFF),
        };
        assert_eq!(bth_unpack(bth_pack(all_ones)), all_ones, "all-ones");
        let zero = Bth::default();
        assert_eq!(bth_unpack(bth_pack(zero)), zero, "all-zeros");
        // One field at a time, to localise a wrong offset.
        for (name, h) in [
            (
                "opcode",
                Bth {
                    opcode: bits(0xA5),
                    ..zero
                },
            ),
            (
                "padcount",
                Bth {
                    padcount: bits(2),
                    ..zero
                },
            ),
            (
                "version",
                Bth {
                    version: bits(0x9),
                    ..zero
                },
            ),
            (
                "pkey",
                Bth {
                    pkey: bits(0x1234),
                    ..zero
                },
            ),
            (
                "resv6",
                Bth {
                    resv6: bits(0x2A),
                    ..zero
                },
            ),
            (
                "dqpn",
                Bth {
                    dqpn: bits(0xAB_CDEF),
                    ..zero
                },
            ),
            (
                "resv7",
                Bth {
                    resv7: bits(0x55),
                    ..zero
                },
            ),
            (
                "psn",
                Bth {
                    psn: bits(0x12_3456),
                    ..zero
                },
            ),
            (
                "solicited",
                Bth {
                    solicited: true,
                    ..zero
                },
            ),
            (
                "migreq",
                Bth {
                    migreq: true,
                    ..zero
                },
            ),
            ("fecn", Bth { fecn: true, ..zero }),
            ("becn", Bth { becn: true, ..zero }),
            (
                "ackreq",
                Bth {
                    ackreq: true,
                    ..zero
                },
            ),
        ] {
            assert_eq!(bth_unpack(bth_pack(h)), h, "field {name} did not survive");
        }
    }

    #[test]
    fn reth_round_trips_through_pack_and_unpack() {
        for h in reths() {
            assert_eq!(reth_unpack(reth_pack(h)), h, "RETH did not survive: {h:?}");
        }
        let all_ones = Reth {
            va: bits(0xFFFF_FFFF_FFFF_FFFF),
            rkey: bits(0xFFFF_FFFF),
            dma_len: bits(0xFFFF_FFFF),
        };
        assert_eq!(reth_unpack(reth_pack(all_ones)), all_ones);
        // Each field alone, so a wrong shift cannot hide.
        let z = Reth::default();
        for (name, h) in [
            (
                "va",
                Reth {
                    va: bits(0x0123_4567_89AB_CDEF),
                    ..z
                },
            ),
            (
                "rkey",
                Reth {
                    rkey: bits(0xDEAD_BEEF),
                    ..z
                },
            ),
            (
                "dma_len",
                Reth {
                    dma_len: bits(0x1234_5678),
                    ..z
                },
            ),
        ] {
            assert_eq!(reth_unpack(reth_pack(h)), h, "field {name} did not survive");
        }
    }

    #[test]
    fn bth_unpack_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = bths().map(|h| (bth_pack(h),));
        test_kernel_vm_and_verilog_synchronous::<bth_unpack, _, _, _>(bth_unpack, inputs)?;
        Ok(())
    }

    #[test]
    fn reth_unpack_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = reths().map(|h| (reth_pack(h),));
        test_kernel_vm_and_verilog_synchronous::<reth_unpack, _, _, _>(reth_unpack, inputs)?;
        Ok(())
    }

    #[test]
    fn bth_pack_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<bth_pack, _, _, _>(
            bth_pack,
            bths().map(|h| (h,)),
        )?;
        Ok(())
    }

    #[test]
    fn bth_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        // Indices past the end included on purpose: the clamp is what
        // keeps the shift inside the operand width.
        let inputs = bths().flat_map(|h| (0u128..16).map(move |i| (h, bits::<4>(i))));
        test_kernel_vm_and_verilog_synchronous::<bth_byte, _, _, _>(bth_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn reth_pack_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<reth_pack, _, _, _>(
            reth_pack,
            reths().map(|h| (h,)),
        )?;
        Ok(())
    }

    #[test]
    fn reth_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = reths().flat_map(|h| (0u128..32).map(move |i| (h, bits::<5>(i))));
        test_kernel_vm_and_verilog_synchronous::<reth_byte, _, _, _>(reth_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn bit_of_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<bit_of<96>, _, _, _>(
            bit_of::<96>,
            [true, false].into_iter().map(|b| (b,)),
        )?;
        Ok(())
    }

    #[test]
    fn carries_reth_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<carries_reth, _, _, _>(
            carries_reth,
            OPS.into_iter().map(|o| (o,)),
        )?;
        Ok(())
    }

    #[test]
    fn transport_base_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<transport_base, _, _, _>(
            transport_base,
            [Transport::Rc, Transport::Uc].into_iter().map(|t| (t,)),
        )?;
        Ok(())
    }

    #[test]
    fn write_operation_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<write_operation, _, _, _>(
            write_operation,
            OPS.into_iter().map(|o| (o,)),
        )?;
        Ok(())
    }

    #[test]
    fn encode_write_opcode_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = [Transport::Rc, Transport::Uc]
            .into_iter()
            .flat_map(|t| OPS.into_iter().map(move |o| (t, o)));
        test_kernel_vm_and_verilog_synchronous::<encode_write_opcode, _, _, _>(
            encode_write_opcode,
            inputs,
        )?;
        Ok(())
    }
}
