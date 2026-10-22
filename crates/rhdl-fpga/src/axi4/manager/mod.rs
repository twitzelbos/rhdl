#![warn(missing_docs)]
//! The manager (master) side: turning a burst descriptor into channel
//! traffic.
//!
//! Phase 2 of `axi4-architecture.md` §8 — one burst at a time, one ID,
//! `Incr` and `Fixed`. The manager is the half of the bus that carries
//! the *obligations*: the 4 KiB rule, `WLAST` position, and legal
//! `AxSIZE` are all things a manager must get right and a subordinate
//! may only detect. That is why these widgets are validated against
//! [`crate::axi4::check`]'s monitors rather than against an endpoint
//! alone — a loopback between two halves of this crate proves they
//! agree, not that either is right.

pub mod write_burst;
