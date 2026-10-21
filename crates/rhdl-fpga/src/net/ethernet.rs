//! The Ethernet II header: fourteen bytes, no VLAN tag.

use super::config::FrameConfig;
use rhdl::prelude::*;

/// EtherType for IPv4.
pub const ETHERTYPE_IPV4: u128 = 0x0800;

/// Length of the Ethernet II header, in bytes. No VLAN tag.
pub const ETH_BYTES: u128 = 14;

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
