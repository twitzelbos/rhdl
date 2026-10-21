//! The Ethernet, IPv4 and UDP parameters of a frame.

use rhdl::prelude::*;

/// The Ethernet, IPv4 and UDP parameters of one flow.
///
/// Constant for the life of a connection, and deliberately *only* the
/// parameters any UDP traffic would have — nothing about whatever
/// protocol rides on top. A protocol's own per-connection state sits
/// beside this in its own type; `rhdl-roce`'s `QpContext` is the
/// worked example.
///
/// # Why one struct rather than three
///
/// [`super::ethernet::eth_byte`] reads only the MAC addresses,
/// [`super::ipv4::ipv4_byte`] only the addresses and the IP fields, and
/// [`super::udp::udp_byte`] only the ports — so three narrower structs
/// would be the purer layering. One is used instead because a frame
/// generator holds all three layers' parameters anyway, and splitting
/// them would put three arguments at every call site to express a thing
/// that is configured, and changes, as a unit.
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
    /// UDP source port. A protocol that leaves this free — RoCEv2 does
    /// varies it to spread traffic across a fabric's ECMP hashes.
    pub src_port: Bits<16>,
    /// UDP destination port — which is what selects the protocol riding
    /// on UDP.
    ///
    /// A parameter rather than a constant inside [`udp_pack`], because
    /// hardcoding 4791 there would make an otherwise generic UDP header
    /// builder quietly specific to one protocol — which is exactly the
    /// coupling that keeps these widgets in `net` rather than in the
    /// crate of whichever protocol first needed them.
    pub dst_port: Bits<16>,
    /// IPv4 identification field. Only meaningful for fragmentation,
    /// which many protocols on UDP forbid — RoCEv2 among them — so a
    /// constant is usually fine.
    pub ip_id: Bits<16>,
    /// IPv4 time to live.
    pub ttl: Bits<8>,
    /// The IPv4 TOS byte — six bits of DSCP then two of ECN.
    ///
    /// Configurable rather than zero because a lossless fabric routinely
    /// depends on it: DCQCN needs ECN-capable transport marked here, and
    /// lossless queues are selected by DSCP. The ICRC masks this byte,
    /// so it does not affect the checksum — but it *is* covered by the
    /// IPv4 header checksum, which is why changing it changes the frame.
    pub dscp_ecn: Bits<8>,
}
