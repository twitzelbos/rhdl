#![warn(missing_docs)]
//! The receive direction: turning frame bytes back into RDMA operations.
//!
//! Where [`crate::tx`] walks a [`crate::qp::segmenter::PacketPlan`] out
//! into bytes, this goes the other way — bytes in, typed headers and a
//! verdict out. The two are deliberately mirror images at the byte
//! interface, so composing them is the Tier-10 loopback test of
//! `roce-v2-architecture.md`.

pub mod endpoint;
pub mod parse;
pub mod verify;
