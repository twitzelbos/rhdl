#![warn(missing_docs)]
//! Ethernet, IPv4 and UDP: the layers underneath whatever protocol is
//! actually being built.
//!
//! These are not interesting in themselves, and that is the point of
//! the category. Every protocol that rides on UDP needs the same
//! fourteen bytes of Ethernet, the same twenty of IPv4 with the same
//! one's-complement checksum, and the same eight of UDP — and needs
//! them both to *emit* and to *parse*. Without somewhere shared to put
//! them, the first protocol to need them puts them in its own crate and
//! the second one copies them.
//!
//! That is not hypothetical: it is exactly what happened. `rhdl-roce`
//! built these as `roce::frame`, then its connection manager needed to
//! parse the same forty-two header bytes its receiver already parsed,
//! and the duplication was committed deliberately with a note saying
//! this category was the right answer and needed sign-off first.
//! This is that category.
//!
//! # What belongs here
//!
//! A widget belongs in `net` when its signature mentions no protocol
//! above UDP. [`ipv4::ipv4_checksum`] qualifies; a function taking a
//! RoCE `PacketPlan` does not, however much of its body is IPv4
//! arithmetic. The test is the *interface*, not the implementation —
//! which is what stops `net` becoming a drawer for anything
//! network-adjacent.
//!
//! # Layout
//!
//! - [`config::FrameConfig`] — the parameters of one flow, shared by
//!   all three layers.
//! - [`ethernet`] — the Ethernet II header.
//! - [`ipv4`] — the IPv4 header and its checksum.
//! - [`udp`] — the UDP header.
//! - [`udp_rx`] — the byte-serial receive parser for all three at
//!   once, which is the piece the duplication was about.
//!
//! A MAC frontend (GMII/RGMII, preamble and SFD, the frame check
//! sequence, the inter-frame gap) is the obvious next resident and is
//! not here yet; `roce-v2-architecture.md` §5.1 is where it is planned.
//! CRC-32 already lives in [`crate::core::crc32`] rather than here,
//! because it is not networking — Ethernet's FCS is one of its users,
//! not its definition.

pub mod config;
pub mod ethernet;
pub mod ipv4;
pub mod udp;
pub mod udp_rx;

pub use config::FrameConfig;
