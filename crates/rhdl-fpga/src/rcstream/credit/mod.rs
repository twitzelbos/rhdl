#![warn(missing_docs)]
//! Credit-based variant of [`super::RCStream<T, F>`] for long-path /
//! multi-source-aggregation use cases.
//!
//! # When to use
//!
//! Per `stream-bus-architecture.md` §11, the credit-based variant is
//! useful when:
//!
//! - **Long inter-block paths** where the sink-to-source `ready`
//!   signal can't meet timing as a combinational input to the source's
//!   TVALID generator.
//! - **Multi-source aggregation** where one sink receives from many
//!   sources and reverse-direction arbitration would be expensive.
//! - **Virtual channels** — one physical SerDes link carrying multiple
//!   logical streams, each with its own credit pool.
//!
//! For ordinary kernel-to-kernel connections within a single design
//! that meet timing, the simple Ready/Valid form
//! ([`super::RCStream`]) is preferred — credit-based adds DFF state
//! at the source for the credit counter and at the sink for the
//! buffer + free-slot tracking.
//!
//! # Type
//!
//! [`CreditRCStream<T, F, CREDIT_W>`] is the connection type.  At
//! the wire level:
//!
//! ```text
//!   source  ──data──>  sink           data: Option<Item<T, F>>
//!   source  <──credit_grant──  sink   credit_grant: Bits<CREDIT_W>
//! ```
//!
//! The `credit_grant` field flows in the same logical direction as
//! `ready` in the simple variant (sink→source), but its semantics
//! are different: instead of "I am ready to accept the next item
//! this cycle", it's "I have granted you this many additional
//! tokens this cycle".  The source maintains a local counter, adds
//! incoming grants, decrements on each item sent, and gates sending
//! on `counter > 0`.
//!
//! Crucially: there is **no combinational path** from `credit_grant`
//! through the source's send decision to `data` within a single
//! cycle.  The source's send decision uses the LATCHED counter (Q
//! value), not the in-cycle `credit_grant`.  This breaks the long
//! TVALID/TREADY combinational dependency that the simple variant
//! has.
//!
//! # Translation widgets
//!
//! Two translator widgets convert between [`super::RCStream`] (the
//! simple variant) and the credit-based protocol:
//!
//! - [`source::CreditSource<T, F, CREDIT_W>`] — converts an
//!   incoming [`super::RCStream`] source into a `CreditRCStream` source.
//!   Tracks the local credit counter; gates outgoing items on
//!   `counter > 0`; signals upstream `ready` when it has credit.
//! - [`sink::CreditSink<T, F, CREDIT_W, FIFO_N>`] — converts an
//!   incoming `CreditRCStream` source into an outgoing
//!   [`super::RCStream`] source.  Buffers items in an internal
//!   `SyncFIFO` of depth `2^FIFO_N`; grants one credit per item
//!   popped (so the source's counter tracks free buffer slots).
//!
//! Together, `CreditSource` → `CreditRCStream` → `CreditSink` form
//! the credit-based pipeline pair.  Insert at long-path / multi-
//! source-aggregation boundaries; the sink's `RCStream` output
//! plugs into the rest of the design unchanged.
//!
//! # Initial credit
//!
//! On reset, the source's credit counter is initialized to 0 and the
//! sink's "credits to grant" is initialized to its buffer depth
//! (`2^FIFO_N`).  The sink emits one credit grant per cycle until
//! its initial pool is drained, then grants additional credits as
//! buffer slots free up.  The source naturally accumulates credits
//! and starts sending once its counter reaches 1.
//!
//! # Lossless conversion
//!
//! Per the design plan §11, conversion between
//! [`super::RCStream<T, F>`] and `CreditRCStream<T, F, CREDIT_W>` is
//! lossless for all framing forms — the framing parameter `F` flows
//! through both translators unchanged.

pub mod sink;
pub mod source;

use rhdl::prelude::*;

use crate::rcstream::bus::Item;

/// A typed, latency-insensitive stream with credit-based flow
/// control.
///
/// At the wire level: `Option<Item<T, F>>` source→sink data signal
/// paired with `Bits<CREDIT_W>` sink→source credit-grant signal.
/// See module docs for semantics + when to use this variant over
/// the simple [`super::RCStream`].
///
/// `CREDIT_W` is the bit-width of the per-cycle credit-grant signal.
/// A larger width lets the sink grant multiple credits per cycle
/// (useful for burst replenishment after a long stall); the source's
/// internal credit counter is also `Bits<CREDIT_W>` wide so the
/// maximum outstanding credit equals `2^CREDIT_W - 1`.
///
/// Typical sizing: `CREDIT_W = 4` (max 15 outstanding credits) for
/// most short-burst use cases; `CREDIT_W = 8` for high-throughput
/// long-path pipelines.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct CreditRCStream<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Source → sink data signal.  Same encoding as
    /// [`super::RCStream::data`]: `None` = idle, `Some(item)` =
    /// item this cycle.
    pub data: Option<Item<T, F>>,
    /// Sink → source credit-grant signal.  Number of additional
    /// tokens the sink grants the source this cycle.  The source
    /// adds these to its local counter (saturating at
    /// `2^CREDIT_W - 1`) and may decrement it by 1 on the same cycle
    /// if it sends an item.
    pub credit_grant: Bits<CREDIT_W>,
}

// Convenience re-exports.
pub use sink::CreditSink;
pub use source::CreditSource;

#[cfg(test)]
mod tests {
    use super::*;

    /// Construction sanity: `CreditRCStream<T, F, CREDIT_W>` is a
    /// valid `Digital` struct that round-trips.
    #[test]
    fn credit_rcstream_construction() {
        let s: CreditRCStream<b8, (), 4> = CreditRCStream {
            data: None,
            credit_grant: bits::<4>(7),
        };
        assert!(s.data.is_none());
        assert_eq!(s.credit_grant.raw(), 7);
    }

    /// `CreditRCStream` carries `Some(item)` like its simple cousin.
    #[test]
    fn credit_rcstream_with_item() {
        let it = Item::<b16, bool> { data: bits::<16>(0xCAFE), frame: true };
        let s: CreditRCStream<b16, bool, 8> = CreditRCStream {
            data: Some(it),
            credit_grant: bits::<8>(1),
        };
        match s.data {
            Some(item) => {
                assert_eq!(item.data.raw(), 0xCAFE);
                assert!(item.frame);
            }
            None => panic!("expected Some(item)"),
        }
        assert_eq!(s.credit_grant.raw(), 1);
    }
}
