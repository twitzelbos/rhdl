//! The IPv4 header: twenty bytes, no options, and its checksum.

use super::config::FrameConfig;
use super::ethernet::ETH_BYTES;
use rhdl::prelude::*;

/// IANA protocol number for UDP.
pub const IP_PROTO_UDP: u128 = 17;

/// Length of an IPv4 header with no options, in bytes.
pub const IPV4_BYTES: u128 = 20;

/// Frame offset of the IPv4 header in an untagged Ethernet II frame.
/// Equal to [`super::ethernet::ETH_BYTES`].
///
/// A protocol whose checksum covers the frame from the IP header
/// onward — RoCEv2's Invariant CRC is one — has to agree with this
/// value, so it is stated here once rather than recomputed.
pub const IPV4_OFFSET: u128 = ETH_BYTES;

#[kernel]
/// Pack IPv4 header bytes 0–11 — everything but the two addresses.
///
/// `checksum` is a parameter rather than being computed here, so that
/// [`ipv4_checksum`] can call this with zero and avoid a circular
/// definition.
pub fn ipv4_head(cfg: FrameConfig, total_len: Bits<16>, checksum: Bits<16>) -> Bits<96> {
    // byte 0      : version 4, IHL 5  -> 0x45
    // byte 1      : TOS / DSCP+ECN
    // bytes 2..3  : total length
    // bytes 4..5  : identification
    // bytes 6..7  : flags + fragment offset -> 0
    // byte 8      : TTL
    // byte 9      : protocol
    // bytes 10..11: header checksum
    (bits::<96>(0x45) << 88)
        | (cfg.dscp_ecn.resize::<96>() << 80)
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
