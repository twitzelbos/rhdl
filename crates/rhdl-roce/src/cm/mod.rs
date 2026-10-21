#![warn(missing_docs)]
//! Connection management: how a queue pair comes to exist.
//!
//! Until this module, a queue pair's parameters had to arrive by
//! "unspecified external means" — `roce-v2-architecture.md`'s own
//! phrase. This is the specification of those means: a 40-byte UDP
//! datagram ([`message`]) and the widget that receives it, applies it
//! and replies ([`manager`]).
//!
//! It is the last piece of Phase 4 because it is the piece that needs
//! everything else: it is a UDP protocol implementation, so it needed
//! a receive path before it could exist at all.

pub mod manager;
pub mod message;
pub mod node;
