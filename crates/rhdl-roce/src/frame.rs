//! The Ethernet, IPv4 and UDP headers that carry a RoCEv2 packet, and
//! the byte-level assembly of a complete frame.
//!
//! RoCEv2's whole trick is that it is ordinary networking underneath: a
//! UDP datagram to the IANA-assigned port 4791. So everything in this
//! module is generic network plumbing rather than anything RDMA-specific.
//!
//! # Where this should eventually live
//!
//! This module holds **two layers**, and they are deliberately kept
//! separable:
//!
//! - **Generic networking**, which belongs in `rhdl-fpga` under a `net`
//!   category: [`FrameConfig`], [`eth_pack`], [`eth_byte`],
//!   [`ipv4_head`], [`ipv4_checksum`], [`ipv4_byte`], [`udp_pack`],
//!   [`udp_byte`] and the layer-offset constants. **None of these
//!   mention an RDMA type.** They take byte *lengths*, so an IPv4
//!   header builder does not need to know what a queue pair is.
//! - **RoCE-specific glue**, which stays here: [`header_byte`]'s
//!   dispatch across the layer boundaries, [`bth_from_plan`], and the
//!   length arithmetic ([`header_bytes`], [`udp_length`],
//!   [`ip_total_length`], [`payload_on_wire`]) that knows a RETH is 16
//!   bytes and that a pad count exists.
//!
//! The first group was *originally written taking a
//! [`PacketPlan`][crate::qp::segmenter::PacketPlan] just to look up a
//! length, which coupled generic code to RDMA for no reason and made it
//! look unmovable. Taking lengths instead is what makes the eventual
//! move to `net` a rename plus an import change.
//!
//! The move itself is not done here, because adding a top-level category
//! to `rhdl-fpga` is one of the structural decisions `architecture.md`
//! requires sign-off for and that sign-off is outstanding
//! (`roce-v2-architecture.md` §12, decision 5).
//!
//! **The assembled transmitter does *not* belong in `net`.**
//! [`crate::tx::RoceWriteTx`] is an RDMA endpoint — queue pairs, packet
//! sequence numbers, remote keys — and `rhdl-fpga` neither depends on
//! this crate nor should. Moving it would mean moving the segmenter, the
//! headers, the ICRC and the queue-pair context too, which is the whole
//! crate.
//!
//! # The frame
//!
//! ```text
//!   offset  0      14          34        42      54            -4
//!           +------+-----------+---------+-------+-------------+------+
//!           | Eth  | IPv4      | UDP     | BTH   | RETH?+pay   | ICRC |
//!           | 14 B | 20 B      | 8 B     | 12 B  |             | 4 B  |
//!           +------+-----------+---------+-------+-------------+------+
//!                                                 ^ 16 B only on FIRST/ONLY
//! ```
//!
//! [`header_byte`] produces any byte of the header block — everything
//! up to the payload — from the connection parameters and a
//! [`PacketPlan`]. The payload comes from a stream and the ICRC from
//! [`crate::icrc`], so those two are the caller's to splice in.
//!
//! # Two things the IBTA and RFC 768 require here
//!
//! - **The UDP checksum is zero.** RoCEv2 mandates it, because the ICRC
//!   already covers the payload and a UDP checksum over the same bytes
//!   would be redundant work at line rate. [`udp_pack`] emits zero and
//!   `tests/vectors.rs` checks that every reference frame agrees.
//! - **The IPv4 header checksum is real** and has to be computed, since
//!   a router will drop a frame whose header checksum is wrong long
//!   before the ICRC is ever looked at. [`ipv4_checksum`] is the
//!   standard one's-complement sum.
//!
//! # Why the IPv4 header is split in two
//!
//! `Bits<N>` is defined for `N` in `1..=128` — see
//! `rhdl_bits::BitWidth` — so the 160-bit IPv4 header cannot be packed
//! into a single value the way the 96-bit BTH and 128-bit RETH are.
//! It is therefore handled as a 96-bit head (bytes 0–11) plus the two
//! 32-bit addresses, which they already are in [`FrameConfig`]. This
//! costs nothing and is worth knowing before reaching for `Bits<160>`.
//!
//! # Clamping, again
//!
//! Every `*_byte` selector clamps its index, and the range dispatch in
//! [`header_byte`] clamps the *sub-index* it hands to each one. That is
//! not belt-and-braces: in a mux **every arm evaluates**, so the
//! arithmetic for the IPv4-address branch runs even when the head
//! branch is selected, and an unclamped `15 - index` there produces a
//! shift past the operand width that the kernel VM rejects outright.
//! CLAUDE.md §4 states the rule; this module is where it bites hardest.

use crate::headers::{Bth, bth_byte, encode_write_opcode, reth_byte};
use crate::qp::context::QpContext;
use crate::qp::segmenter::PacketPlan;
use rhdl::prelude::*;

/// EtherType for IPv4.
pub const ETHERTYPE_IPV4: u128 = 0x0800;
/// The IANA-assigned UDP destination port for RoCEv2.
pub const ROCE_UDP_PORT: u128 = 4791;
/// IANA protocol number for UDP.
pub const IP_PROTO_UDP: u128 = 17;

/// Length of the Ethernet II header, in bytes. No VLAN tag.
pub const ETH_BYTES: u128 = 14;
/// Length of an IPv4 header with no options, in bytes.
pub const IPV4_BYTES: u128 = 20;
/// Length of the UDP header, in bytes.
pub const UDP_BYTES: u128 = 8;
/// Length of the ICRC trailer, in bytes.
pub const ICRC_BYTES: u128 = 4;

/// Frame offset of the IPv4 header. Equal to [`ETH_BYTES`], and the
/// value [`crate::icrc::COVERAGE_START`] has to agree with.
pub const IPV4_OFFSET: u128 = ETH_BYTES;
/// Frame offset of the UDP header.
pub const UDP_OFFSET: u128 = IPV4_OFFSET + IPV4_BYTES;
/// Frame offset of the BTH.
pub const BTH_OFFSET: u128 = UDP_OFFSET + UDP_BYTES;
/// Frame offset of the RETH, when the opcode carries one.
pub const RETH_OFFSET: u128 = BTH_OFFSET + 12;

/// The link and network parameters of one connection.
///
/// Constant for the life of the connection, alongside
/// [`QpContext`]: that holds the RDMA-level parameters and this holds
/// the ones that would be the same for any UDP traffic. Both are
/// written by the out-of-band connection manager.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct FrameConfig {
    /// Destination MAC — the peer NIC's, learned at connection setup.
    pub dst_mac: Bits<48>,
    /// This interface's MAC.
    pub src_mac: Bits<48>,
    /// This interface's IPv4 address.
    pub src_ip: Bits<32>,
    /// The peer's IPv4 address.
    pub dst_ip: Bits<32>,
    /// UDP source port. RoCEv2 leaves this free, and a real sender
    /// varies it to spread traffic across a fabric's ECMP hashes.
    pub src_port: Bits<16>,
    /// UDP destination port. [`ROCE_UDP_PORT`] for RoCEv2.
    ///
    /// A parameter rather than a constant inside [`udp_pack`], because
    /// hardcoding 4791 there would make an otherwise generic UDP header
    /// builder quietly RoCE-specific — which is the coupling the module
    /// docs are about.
    pub dst_port: Bits<16>,
    /// IPv4 identification field. Only meaningful for fragmentation,
    /// which RoCEv2 must not use, so a constant is fine.
    pub ip_id: Bits<16>,
    /// IPv4 time to live.
    pub ttl: Bits<8>,
}

#[kernel]
/// Payload bytes actually on the wire, including the pad bytes.
///
/// Distinct from `plan.payload_bytes`, which is the *data* length. The
/// difference is `padcount`, and conflating the two makes every length
/// field in the frame wrong by up to three.
pub fn payload_on_wire(plan: PacketPlan) -> Bits<13> {
    plan.payload_bytes + plan.padcount.resize::<13>()
}

#[kernel]
/// Length of the header block — everything before the payload.
///
/// 54 bytes, or 70 when the opcode carries a RETH.
pub fn header_bytes(plan: PacketPlan) -> Bits<7> {
    if plan.carries_reth {
        bits(70)
    } else {
        bits(54)
    }
}

#[kernel]
/// The UDP length field: the UDP header plus everything after it.
pub fn udp_length(plan: PacketPlan) -> Bits<16> {
    let ext = if plan.carries_reth {
        bits::<16>(16)
    } else {
        bits::<16>(0)
    };
    // UDP header + BTH + ICRC, plus the RETH and the padded payload.
    bits::<16>(24) + ext + payload_on_wire(plan).resize::<16>()
}

#[kernel]
/// The IPv4 total-length field: the IPv4 header plus the UDP datagram.
pub fn ip_total_length(plan: PacketPlan) -> Bits<16> {
    bits::<16>(20) + udp_length(plan)
}

#[kernel]
/// Pack the Ethernet II header into its 112 wire bits.
pub fn eth_pack(cfg: FrameConfig) -> Bits<112> {
    (cfg.dst_mac.resize::<112>() << 64) | (cfg.src_mac.resize::<112>() << 16) | bits(0x0800)
}

#[kernel]
/// Select one byte of the Ethernet header. `index` is clamped.
pub fn eth_byte(cfg: FrameConfig, index: Bits<4>) -> Bits<8> {
    let safe = if index > bits(13) {
        bits::<4>(13)
    } else {
        index
    };
    let shift = (bits::<8>(13) - safe.resize::<8>()) << 3;
    ((eth_pack(cfg) >> shift.resize::<112>()) & bits(0xFF)).resize::<8>()
}

#[kernel]
/// Pack IPv4 header bytes 0–11 — everything but the two addresses.
///
/// `checksum` is a parameter rather than being computed here, so that
/// [`ipv4_checksum`] can call this with zero and avoid a circular
/// definition.
pub fn ipv4_head(cfg: FrameConfig, total_len: Bits<16>, checksum: Bits<16>) -> Bits<96> {
    // byte 0      : version 4, IHL 5  -> 0x45
    // byte 1      : TOS / DSCP+ECN    -> 0
    // bytes 2..3  : total length
    // bytes 4..5  : identification
    // bytes 6..7  : flags + fragment offset -> 0
    // byte 8      : TTL
    // byte 9      : protocol
    // bytes 10..11: header checksum
    (bits::<96>(0x45) << 88)
        | (total_len.resize::<96>() << 64)
        | (cfg.ip_id.resize::<96>() << 48)
        | (cfg.ttl.resize::<96>() << 24)
        | (bits::<96>(17) << 16)
        | checksum.resize::<96>()
}

#[kernel]
/// The IPv4 header checksum: the one's-complement sum of the header's
/// ten 16-bit words, complemented, computed with the checksum field
/// taken as zero.
pub fn ipv4_checksum(cfg: FrameConfig, total_len: Bits<16>) -> Bits<16> {
    let head = ipv4_head(cfg, total_len, bits(0));
    let mut acc = bits::<32>(0);
    // Six words from the head; the checksum word is zero in it.
    for i in 0..6 {
        let shift = ((5 - i) * 16) as u128;
        acc += ((head >> shift) & bits::<96>(0xFFFF)).resize::<32>();
    }
    // Four more from the two addresses.
    acc += ((cfg.src_ip >> 16) & bits(0xFFFF)).resize::<32>();
    acc += (cfg.src_ip & bits(0xFFFF)).resize::<32>();
    acc += ((cfg.dst_ip >> 16) & bits(0xFFFF)).resize::<32>();
    acc += (cfg.dst_ip & bits(0xFFFF)).resize::<32>();
    // Fold the carries in. Twice, because the first fold can itself
    // carry out of bit 15.
    let fold1 = (acc & bits(0xFFFF)) + (acc >> 16);
    let fold2 = (fold1 & bits(0xFFFF)) + (fold1 >> 16);
    ((fold2 & bits(0xFFFF)) ^ bits(0xFFFF)).resize::<16>()
}

#[kernel]
/// Select one byte of the IPv4 header. `index` is clamped, and so is
/// the sub-index handed to each of the three ranges — see the module
/// docs on why that is load-bearing rather than defensive.
pub fn ipv4_byte(cfg: FrameConfig, total_len: Bits<16>, index: Bits<5>) -> Bits<8> {
    let safe = if index > bits(19) {
        bits::<5>(19)
    } else {
        index
    };
    let in_head = safe < bits(12);
    let in_src = safe >= bits(12) && safe < bits(16);

    // Each sub-index is clamped into its own range *before* being used
    // as a shift, because all three branches below evaluate.
    let head_i = if in_head { safe } else { bits::<5>(11) };
    let src_i = if in_src {
        safe - bits(12)
    } else {
        bits::<5>(0)
    };
    let dst_i = if safe >= bits(16) {
        safe - bits(16)
    } else {
        bits::<5>(0)
    };

    let head_shift = (bits::<8>(11) - head_i.resize::<8>()) << 3;
    let src_shift = (bits::<8>(3) - src_i.resize::<8>()) << 3;
    let dst_shift = (bits::<8>(3) - dst_i.resize::<8>()) << 3;

    let head = ipv4_head(cfg, total_len, ipv4_checksum(cfg, total_len));
    let from_head = ((head >> head_shift.resize::<96>()) & bits(0xFF)).resize::<8>();
    let from_src = ((cfg.src_ip >> src_shift.resize::<32>()) & bits(0xFF)).resize::<8>();
    let from_dst = ((cfg.dst_ip >> dst_shift.resize::<32>()) & bits(0xFF)).resize::<8>();

    if in_head {
        from_head
    } else if in_src {
        from_src
    } else {
        from_dst
    }
}

#[kernel]
/// Pack the UDP header into its 64 wire bits.
///
/// The checksum is zero, which RoCEv2 requires.
pub fn udp_pack(cfg: FrameConfig, udp_len: Bits<16>) -> Bits<64> {
    (cfg.src_port.resize::<64>() << 48)
        | (cfg.dst_port.resize::<64>() << 32)
        | (udp_len.resize::<64>() << 16)
}

#[kernel]
/// Select one byte of the UDP header. `index` is clamped.
pub fn udp_byte(cfg: FrameConfig, udp_len: Bits<16>, index: Bits<3>) -> Bits<8> {
    let safe = if index > bits(7) { bits::<3>(7) } else { index };
    let shift = (bits::<8>(7) - safe.resize::<8>()) << 3;
    ((udp_pack(cfg, udp_len) >> shift.resize::<64>()) & bits(0xFF)).resize::<8>()
}

#[kernel]
/// Build the Base Transport Header for a planned packet.
///
/// `ackreq` is a parameter rather than a [`PacketPlan`] field because
/// it is a reliable-transport concern: [`crate::headers::Transport::Uc`]
/// never acknowledges, so the send queue has no opinion about it yet.
/// When the RC phase lands it becomes the requester's decision about
/// which packets to ask for acknowledgement on.
pub fn bth_from_plan(ctx: QpContext, plan: PacketPlan, ackreq: bool) -> Bth {
    let mut h = Bth::dont_care();
    h.opcode = encode_write_opcode(ctx.transport, plan.opcode);
    h.solicited = false;
    h.migreq = false;
    h.padcount = plan.padcount;
    h.version = bits(0);
    h.pkey = ctx.pkey;
    // Both ECN bits are the fabric's to set, never the sender's.
    h.fecn = false;
    h.becn = false;
    h.resv6 = bits(0);
    h.dqpn = ctx.dqpn;
    h.ackreq = ackreq;
    h.resv7 = bits(0);
    h.psn = plan.psn;
    h
}

#[kernel]
/// Any byte of the frame's header block, counting from the first byte
/// on the wire.
///
/// Covers Ethernet, IPv4, UDP, BTH and — when the opcode carries one —
/// the RETH. The payload and the ICRC are not here: the payload comes
/// from a stream and the ICRC from [`crate::icrc`].
///
/// The [`Bth`] is a **parameter rather than being built here**, for two
/// reasons. It is constant across a packet's bytes, so building it per
/// index would describe the same logic once per byte and leave the
/// sharing to the synthesiser; and it keeps this function to four
/// arguments, which is what `TestArg` supports — so the top-level byte
/// generator can have a kernel-VM and Verilog round-trip instead of
/// being the one function in the path that never sees a Verilog
/// compiler. Build it with [`bth_from_plan`].
///
/// `index` beyond [`header_bytes`] returns the last RETH byte. That is
/// a clamp rather than an error because a mux has no error arm; the
/// caller is responsible for not asking. Note that the clamp has to be
/// applied *before* the sub-index is narrowed — `resize` truncates
/// rather than saturating, so clamping afterwards lets a large index
/// wrap around to byte 0.
pub fn header_byte(cfg: FrameConfig, bth: Bth, plan: PacketPlan, index: Bits<7>) -> Bits<8> {
    // As everywhere in this module: every branch evaluates, so each
    // sub-index is clamped into its own range before use.
    let eth_i = if index < bits(14) {
        index.resize::<4>()
    } else {
        bits::<4>(13)
    };
    let ip_i = if index >= bits(14) && index < bits(34) {
        (index - bits(14)).resize::<5>()
    } else {
        bits::<5>(19)
    };
    let udp_i = if index >= bits(34) && index < bits(42) {
        (index - bits(34)).resize::<3>()
    } else {
        bits::<3>(7)
    };
    let bth_i = if index >= bits(42) && index < bits(54) {
        (index - bits(42)).resize::<4>()
    } else {
        bits::<4>(11)
    };
    // Clamp *before* narrowing: `(index - 54).resize::<5>()` truncates
    // rather than saturating, so index 86 would wrap to RETH byte 0
    // instead of the last byte. Found by the out-of-range selector test.
    let reth_off = if index >= bits(54) {
        index - bits(54)
    } else {
        bits::<7>(15)
    };
    let reth_i = if reth_off > bits(15) {
        bits::<5>(15)
    } else {
        reth_off.resize::<5>()
    };

    if index < bits(14) {
        eth_byte(cfg, eth_i)
    } else if index < bits(34) {
        ipv4_byte(cfg, ip_total_length(plan), ip_i)
    } else if index < bits(42) {
        udp_byte(cfg, udp_length(plan), udp_i)
    } else if index < bits(54) {
        bth_byte(bth, bth_i)
    } else {
        reth_byte(plan.reth, reth_i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::{Transport, WriteOpcode};
    use crate::qp::context::Pmtu;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;

    fn cfg(src_ip: u128, dst_ip: u128, ip_id: u128, ttl: u128) -> FrameConfig {
        FrameConfig {
            dst_mac: bits(0xec0d_9aaa_bbcc),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(src_ip),
            dst_ip: bits(dst_ip),
            src_port: bits(0x1234),
            dst_port: bits(ROCE_UDP_PORT),
            ip_id: bits(ip_id),
            ttl: bits(ttl),
        }
    }

    /// A plan whose IPv4 total length is 76 — the same shape as the
    /// `rc_write_only_16` vector.
    fn plan16() -> PacketPlan {
        PacketPlan {
            opcode: WriteOpcode::Only,
            psn: bits(1),
            payload_bytes: bits(16),
            padcount: bits(0),
            carries_reth: true,
            reth: crate::headers::Reth {
                va: bits(0x0000_7F00_1234_0000),
                rkey: bits(0x0000_A5A5),
                dma_len: bits(16),
            },
        }
    }

    fn ctx() -> QpContext {
        QpContext {
            dqpn: bits(0x000111),
            transport: Transport::Rc,
            pmtu: Pmtu::B1024,
            rkey: bits(0x0000_A5A5),
            pkey: bits(0xFFFF),
        }
    }

    /// The textbook IPv4 checksum: sum the ten 16-bit words, fold the
    /// carries **in a loop** until none remain, complement. Written from
    /// RFC 1071 rather than from the kernel, so agreement means
    /// something.
    fn checksum_ref(c: FrameConfig, p: PacketPlan) -> u16 {
        let src = c.src_ip.raw() as u32;
        let dst = c.dst_ip.raw() as u32;
        let words: [u32; 10] = [
            0x4500,
            ip_total_length(p).raw() as u32,
            c.ip_id.raw() as u32,
            0x0000,
            ((c.ttl.raw() as u32) << 8) | 17,
            0x0000, // the checksum field itself
            src >> 16,
            src & 0xFFFF,
            dst >> 16,
            dst & 0xFFFF,
        ];
        let mut acc: u32 = words.iter().sum();
        while acc >> 16 != 0 {
            acc = (acc & 0xFFFF) + (acc >> 16);
        }
        (acc ^ 0xFFFF) as u16
    }

    /// Does the first carry fold itself carry out of bit 15? That is the
    /// case the *second* fold exists for, and the case the committed
    /// Scapy vectors happen never to produce.
    fn needs_second_fold(c: FrameConfig, p: PacketPlan) -> bool {
        let src = c.src_ip.raw() as u32;
        let dst = c.dst_ip.raw() as u32;
        let acc: u32 = 0x4500
            + ip_total_length(p).raw() as u32
            + c.ip_id.raw() as u32
            + (((c.ttl.raw() as u32) << 8) | 17)
            + (src >> 16)
            + (src & 0xFFFF)
            + (dst >> 16)
            + (dst & 0xFFFF);
        (acc & 0xFFFF) + (acc >> 16) > 0xFFFF
    }

    /// The checksum agrees with RFC 1071's loop, **including on inputs
    /// that need the second carry fold**.
    ///
    /// This test exists because a mutation that deleted the second fold
    /// passed every other test in this crate: the eight committed Scapy
    /// vectors never drive the accumulator into the window where the
    /// first fold carries. The cases below were found by solving for it
    /// — `acc` has to land in `0x1FFFF`, `0x2FFFE..=0x2FFFF`,
    /// `0x3FFFD..=0x3FFFF` and so on — and the count assertion at the
    /// end is what stops this test silently becoming vacuous again.
    #[test]
    fn checksum_agrees_with_rfc1071_including_the_double_carry() {
        // The first four are solved to force the second fold; the rest
        // are ordinary addresses.
        let cases = [
            (0x7AA3_0000u128, 0x0000_0000u128, 0xFFFFu128, 64u128),
            (0xFFFF_7AA3, 0x0000_0000, 0xFFFF, 64),
            (0xFFFF_7AA4, 0x0000_0000, 0xFFFF, 64),
            (0xFFFF_FFFF, 0x7AA3_0000, 0xFFFF, 64),
            (0xC0A8_0132, 0xC0A8_010A, 0x0000, 64),
            (0x0A00_0001, 0x0A00_0002, 0x1234, 255),
            (0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, 255),
            (0x0000_0000, 0x0000_0000, 0x0000, 1),
            (0x8000_0000, 0x0000_0001, 0xABCD, 128),
        ];
        let mut forced = 0;
        for (src, dst, id, ttl) in cases {
            let c = cfg(src, dst, id, ttl);
            let p = plan16();
            assert_eq!(
                ipv4_checksum(c, ip_total_length(p)).raw() as u16,
                checksum_ref(c, p),
                "checksum mismatch for src={src:#010x} dst={dst:#010x} id={id:#06x} ttl={ttl}"
            );
            if needs_second_fold(c, p) {
                forced += 1;
            }
        }
        assert!(
            forced >= 4,
            "only {forced} cases exercised the second carry fold; this test \
             is the only thing covering it, so it must not go vacuous"
        );
    }

    /// The property every IPv4 header must satisfy: with the computed
    /// checksum in place, the one's-complement sum over the whole header
    /// is `0xFFFF`.
    #[test]
    fn a_header_carrying_its_checksum_sums_to_all_ones() {
        for (src, dst, id, ttl) in [
            (0xC0A8_0132u128, 0xC0A8_010Au128, 0x0000u128, 64u128),
            (0x7AA3_0000, 0x0000_0000, 0xFFFF, 64),
            (0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, 255),
        ] {
            let c = cfg(src, dst, id, ttl);
            let p = plan16();
            let mut acc: u32 = 0;
            for i in 0..10u128 {
                let hi = ipv4_byte(c, ip_total_length(p), bits(2 * i)).raw() as u32;
                let lo = ipv4_byte(c, ip_total_length(p), bits(2 * i + 1)).raw() as u32;
                acc += (hi << 8) | lo;
            }
            while acc >> 16 != 0 {
                acc = (acc & 0xFFFF) + (acc >> 16);
            }
            assert_eq!(
                acc, 0xFFFF,
                "header for src={src:#010x} does not sum to all-ones"
            );
        }
    }

    /// RoCEv2 requires a zero UDP checksum. Pinned, because emitting a
    /// real one would be both wasted work and a protocol violation.
    #[test]
    fn the_udp_checksum_is_zero() {
        let c = cfg(0xC0A8_0132, 0xC0A8_010A, 0, 64);
        assert_eq!(udp_byte(c, udp_length(plan16()), bits(6)), bits::<8>(0));
        assert_eq!(udp_byte(c, udp_length(plan16()), bits(7)), bits::<8>(0));
    }

    /// The destination port is the IANA-assigned one, in the right byte
    /// order.
    #[test]
    fn the_destination_port_is_4791() {
        let c = cfg(0xC0A8_0132, 0xC0A8_010A, 0, 64);
        let hi = udp_byte(c, udp_length(plan16()), bits(2)).raw();
        let lo = udp_byte(c, udp_length(plan16()), bits(3)).raw();
        assert_eq!((hi << 8) | lo, ROCE_UDP_PORT);
    }

    /// Length arithmetic, including the padded-payload distinction that
    /// is easy to conflate.
    #[test]
    fn length_fields_account_for_padding() {
        let mut p = plan16();
        p.payload_bytes = bits(13);
        p.padcount = bits(3);
        assert_eq!(payload_on_wire(p), bits::<13>(16));
        // 8 UDP + 12 BTH + 16 RETH + 16 padded payload + 4 ICRC
        assert_eq!(udp_length(p), bits::<16>(56));
        assert_eq!(ip_total_length(p), bits::<16>(76));
        // Without a RETH the frame is 16 bytes shorter.
        p.carries_reth = false;
        assert_eq!(udp_length(p), bits::<16>(40));
        assert_eq!(header_bytes(p), bits::<7>(54));
    }

    /// Every selector clamps, so an out-of-range index cannot trip the
    /// kernel VM's shift bound. See the module docs.
    #[test]
    fn selectors_clamp_out_of_range_indices() {
        let c = cfg(0xC0A8_0132, 0xC0A8_010A, 0, 64);
        let p = plan16();
        let last_eth = eth_byte(c, bits(13));
        for i in 14u128..16 {
            assert_eq!(eth_byte(c, bits(i)), last_eth, "eth_byte({i})");
        }
        let last_ip = ipv4_byte(c, ip_total_length(p), bits(19));
        for i in 20u128..32 {
            assert_eq!(
                ipv4_byte(c, ip_total_length(p), bits(i)),
                last_ip,
                "ipv4_byte({i})"
            );
        }
        // And the frame-level dispatcher, past the header block.
        let bth = bth_from_plan(ctx(), p, false);
        let last_hdr = header_byte(c, bth, p, bits(69));
        for i in 70u128..128 {
            assert_eq!(
                header_byte(c, bth, p, bits(i)),
                last_hdr,
                "header_byte({i})"
            );
        }
    }

    /// The frame offsets this module exports agree with the ICRC's
    /// coverage start. Two modules naming the same boundary is exactly
    /// how they drift.
    #[test]
    fn the_layer_offsets_agree_with_the_icrc_module() {
        assert_eq!(IPV4_OFFSET, crate::icrc::COVERAGE_START);
        assert_eq!(IPV4_OFFSET, 14);
        assert_eq!(UDP_OFFSET, 34);
        assert_eq!(BTH_OFFSET, 42);
        assert_eq!(RETH_OFFSET, 58 - 4);
    }

    // ---------------- Tier 3 + 4 for combinational kernels ----------------
    //
    // Per the CHANGELOG's note: for a pure combinational kernel,
    // `test_kernel_vm_and_verilog_synchronous` is the right Tier 3+4
    // cover -- it lowers to Verilog, runs both the Rust VM and
    // `iverilog`, and compares per input.

    /// A spread of plans: both RETH states, all four pad counts, and
    /// payload lengths across the 13-bit range.
    fn plans() -> impl Iterator<Item = PacketPlan> + Clone {
        (0u128..40).map(|i| {
            let data = (i * 211) % 4097;
            PacketPlan {
                opcode: match i % 6 {
                    0 => WriteOpcode::First,
                    1 => WriteOpcode::Middle,
                    2 => WriteOpcode::Last,
                    3 => WriteOpcode::LastWithImmediate,
                    4 => WriteOpcode::Only,
                    _ => WriteOpcode::OnlyWithImmediate,
                },
                psn: bits((i * 99_991) & 0xFF_FFFF),
                payload_bytes: bits(data),
                padcount: bits((4 - data % 4) % 4),
                carries_reth: i % 2 == 0,
                reth: crate::headers::Reth {
                    va: bits((i * 0x0123_4567_89AB_CDEF) & u64::MAX as u128),
                    rkey: bits((i * 0x0101_0101) & 0xFFFF_FFFF),
                    dma_len: bits(data),
                },
            }
        })
    }

    fn cfgs() -> impl Iterator<Item = FrameConfig> + Clone {
        (0u128..16).map(|i| {
            cfg(
                0xC0A8_0100 + i * 37,
                0x0A00_0001 + i * 1021,
                i * 4099,
                i * 17,
            )
        })
    }

    #[test]
    fn eth_pack_kernel_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<eth_pack, _, _, _>(
            eth_pack,
            cfgs().map(|c| (c,)),
        )?;
        Ok(())
    }

    #[test]
    fn ipv4_head_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = cfgs()
            .zip(plans())
            .map(|(c, p)| (c, ip_total_length(p), bits::<16>(0xBEEF)));
        test_kernel_vm_and_verilog_synchronous::<ipv4_head, _, _, _>(ipv4_head, inputs)?;
        Ok(())
    }

    #[test]
    fn udp_pack_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = cfgs().zip(plans()).map(|(c, p)| (c, udp_length(p)));
        test_kernel_vm_and_verilog_synchronous::<udp_pack, _, _, _>(udp_pack, inputs)?;
        Ok(())
    }

    #[test]
    fn udp_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        // Indices past the end included: the clamp bounds the shift.
        let inputs = cfgs().zip(plans()).flat_map(|(c, p)| {
            let ul = udp_length(p);
            (0u128..8).map(move |i| (c, ul, bits::<3>(i)))
        });
        test_kernel_vm_and_verilog_synchronous::<udp_byte, _, _, _>(udp_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn bth_from_plan_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = plans().flat_map(|p| [true, false].into_iter().map(move |a| (ctx(), p, a)));
        test_kernel_vm_and_verilog_synchronous::<bth_from_plan, _, _, _>(bth_from_plan, inputs)?;
        Ok(())
    }

    /// **The top-level byte generator, through the kernel VM and
    /// `iverilog`.** This is the one that mattered: every byte-exact
    /// claim about whole frames -- eight curated vectors and 2000 fuzz
    /// cases -- runs through direct Rust calls to this function, which
    /// §4 warns are more permissive than the VM. Without this test the
    /// whole-frame result was a statement about the Rust simulator and
    /// not about hardware.
    #[test]
    fn header_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = cfgs().zip(plans()).flat_map(|(c, p)| {
            let bth = bth_from_plan(ctx(), p, true);
            // 0..72 so the out-of-range clamp is exercised in Verilog too.
            (0u128..72).map(move |i| (c, bth, p, bits::<7>(i)))
        });
        test_kernel_vm_and_verilog_synchronous::<header_byte, _, _, _>(header_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn length_kernels_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<payload_on_wire, _, _, _>(
            payload_on_wire,
            plans().map(|p| (p,)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<header_bytes, _, _, _>(
            header_bytes,
            plans().map(|p| (p,)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<udp_length, _, _, _>(
            udp_length,
            plans().map(|p| (p,)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<ip_total_length, _, _, _>(
            ip_total_length,
            plans().map(|p| (p,)),
        )?;
        Ok(())
    }

    #[test]
    fn checksum_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..64).map(|i| {
            (
                cfg(0xC0A8_0100 + i, 0xC0A8_0200 + i * 7, i * 1021, 64),
                ip_total_length(plan16()),
            )
        });
        test_kernel_vm_and_verilog_synchronous::<ipv4_checksum, _, _, _>(ipv4_checksum, inputs)?;
        Ok(())
    }

    #[test]
    fn ipv4_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..32).map(|i| {
            (
                cfg(0xC0A8_0132, 0xC0A8_010A, 0x1234, 64),
                ip_total_length(plan16()),
                bits::<5>(i),
            )
        });
        test_kernel_vm_and_verilog_synchronous::<ipv4_byte, _, _, _>(ipv4_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn eth_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..16).map(|i| (cfg(0, 0, 0, 64), bits::<4>(i)));
        test_kernel_vm_and_verilog_synchronous::<eth_byte, _, _, _>(eth_byte, inputs)?;
        Ok(())
    }
}
