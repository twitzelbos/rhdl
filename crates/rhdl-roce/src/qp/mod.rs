#![warn(missing_docs)]
//! Queue-pair state: the connection parameters and the message-to-packet
//! segmenter.
//!
//! A *queue pair* is RDMA's connection abstraction. [`context`] holds
//! the parameters that do not change for the life of one — the
//! destination queue-pair number, the transport service, the negotiated
//! path MTU and the remote key — and [`segmenter`] turns an
//! application's message into the sequence of packets the path MTU
//! requires.
//!
//! This is the send side only, and the unreliable-connection subset of
//! it: there is no acknowledgement path, no sequence-number window and
//! no retransmission buffer, because [`context::QpContext`] with
//! [`crate::headers::Transport::Uc`] does not need them. See
//! `roce-v2-architecture.md` §5.3 for why that transport comes first
//! and §3 for the measurement showing that reliable transport is
//! roughly a third of a full implementation.

pub mod context;
pub mod segmenter;
