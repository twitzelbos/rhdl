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

pub mod read_burst;
pub mod write_burst;

use super::types::{
    AxiAddress, BurstKind, bytes_per_beat, crosses_4k, size_fits_bus, wrap_len_is_legal,
};
use rhdl::prelude::*;

#[kernel]
/// Can a manager drive this burst *conformantly*?
///
/// Shared by both managers, and shared deliberately: the 4 KiB rule,
/// `Wrap` legality and `AxSIZE` constrain AW and AR identically, so two
/// copies would be two places to fix a misreading and a drift between
/// them would leave both managers' tests passing. The monitors share
/// their address-phase rules for exactly the same reason — see
/// [`crate::axi4::check::rules::address_fault`].
///
/// Everything this refuses is something the manager would otherwise
/// drive as a **protocol violation**. The 4 KiB rule, `Wrap` legality
/// and `AxSIZE` are manager obligations a subordinate may only detect,
/// never fix, so refusing is the only correct answer; driving them
/// would make the manager the non-conformant master the monitors exist
/// to catch.
///
/// - `Fixed`, and `Incr` within a 4 KiB page: drivable. An unaligned
///   `Incr` start is legal, and the write manager's `lane_strobe`
///   reduces its first beat accordingly.
/// - `Incr` crossing a 4 KiB boundary: refused. The spec forbids it,
///   and it is the rule the literature reports violated most often.
/// - `Wrap`: drivable only with `len ∈ {2,4,8,16}` and a size-aligned
///   start, both of which the spec requires.
/// - Any `AxSIZE` wider than the bus: refused.
pub fn drivable<const A: usize, const ID: usize>(a: AxiAddress<A, ID>, bus_bytes: Bits<9>) -> bool
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let bytes = bytes_per_beat(a.size).resize::<A>();
    let aligned = (a.addr & (bytes - bits::<A>(1))) == bits::<A>(0);
    let kind_ok = match a.burst {
        BurstKind::Incr => !crosses_4k::<A>(a.addr, a.size, a.len, a.burst),
        BurstKind::Fixed => true,
        BurstKind::Wrap => wrap_len_is_legal(a.len) && aligned,
    };
    kind_ok && size_fits_bus(a.size, bus_bytes)
}
