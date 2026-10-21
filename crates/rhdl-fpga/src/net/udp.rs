//! The UDP header: eight bytes.

use super::config::FrameConfig;
use super::ipv4::{IPV4_BYTES, IPV4_OFFSET};
use rhdl::prelude::*;

/// Length of the UDP header, in bytes.
pub const UDP_BYTES: u128 = 8;

/// Frame offset of the UDP header.
pub const UDP_OFFSET: u128 = IPV4_OFFSET + IPV4_BYTES;

#[kernel]
/// Pack the UDP header into its 64 wire bits.
///
/// The checksum is emitted as zero, which IPv4 permits and which some
/// protocols on UDP require — RoCEv2 among them, because its own
/// Invariant CRC covers the same bytes and a UDP checksum would be
/// redundant with it.
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
