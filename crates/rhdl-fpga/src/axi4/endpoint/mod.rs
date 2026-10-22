#![warn(missing_docs)]
//! The subordinate (endpoint) side: serving bursts out of memory.
//!
//! Phase 2 of `axi4-architecture.md` §8. A subordinate's job with the
//! manager's obligations — the 4 KiB rule, `WLAST` position, legal
//! `AxSIZE` — is to **detect and report**, never to fix; that is what
//! [`crate::axi4::check`] is for. What an endpoint owes in return is
//! that the bytes it stores are the bytes it was sent, which is what
//! Phase 2's byte-exact loopback gate measures.

pub mod mem;
