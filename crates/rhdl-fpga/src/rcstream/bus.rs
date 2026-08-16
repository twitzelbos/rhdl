#![warn(missing_docs)]
//! `RCStream<T, F>` — the canonical typed, latency-insensitive
//! streaming bus for RHDL.
//!
//! The "RC" prefix names the two design properties the bus inherits:
//! **R**HDL's type system and **C**arloni's latency-insensitive-design
//! theorem.  The bus type carries a payload `T`, an optional framing
//! marker `F`, and a `bool` ready signal in the opposite direction.
//! It is correct-by-construction under arbitrary pipeline insertion
//! ([`super::relay::RCStreamRelay`] is its native pipeline-stage
//! primitive), drops or replaces every awkwardness of AXI4-Stream,
//! and falls out naturally from RHDL's existing type system.
//!
//! See `stream-bus-architecture.md` at the repository root for the
//! design plan + AXI4-Stream comparison + framing-pattern catalogue.
//!
//!# Wire encoding
//!
//! ```text
//!   source  ──data───>  sink         data: Option<Item<T, F>>
//!   source  <──ready──  sink         ready: bool
//! ```
//!
//! - `data: Option<Item<T, F>>`.  `None` = idle (TVALID = 0); `Some(item)`
//!   = data this cycle (TVALID = 1).  The validity bit IS the
//!   `Option`'s discriminant; there is no separate `valid` signal.
//! - `ready: bool`.  `true` = ready to accept the next item.  Following
//!   the AXI4-Stream contract: `ready` MAY depend combinationally on
//!   `data.is_some()`, but `data.is_some()` MUST NOT depend
//!   combinationally on `ready`.  This is what makes the
//!   [`super::relay::RCStreamRelay`] insertion sound.
//!
//!# Framing parameter `F`
//!
//! `F` is the type of the framing marker.  Common idioms:
//!
//! | `F` | AXI4-Stream equivalent | Use case |
//! |---|---|---|
//! | `()` | TLAST = 0, TUSER unused | Pure data stream, no framing |
//! | `bool` | TLAST | End-of-frame marker |
//! | `Channel` (Digital enum) | TDEST | Multi-channel multiplex |
//! | `b8` etc. | (no equivalent) | Sequence numbering |
//! | struct with last/seq/error/etc. | TUSER | Sideband flags |
//!
//! The framing semantics are part of the type — mismatched `F`
//! between two ends of a connection is a compile error, not a silent
//! TUSER mismatch.
//!
//!# `RCStream` vs AXI4-Stream
//!
//! See `stream-bus-architecture.md` §15 for the load-bearing comparison
//! table.  Highlights: typed payload (no TDATA bit-pack), typed framing
//! (no TUSER), no TKEEP/TSTRB byte-keep ambiguity (use typed payload),
//! cross-IP TUSER mismatches become compile errors instead of "the
//! simulation worked but the silicon doesn't" bugs.
//!
//!# Synchronous-widget convention
//!
//! `RCStream<T, F>` is used as a Synchronous-widget I/O type via the
//! convention:
//!
//! - When used as widget **I** (`SynchronousIO::I`):
//!     - `data` is the upstream's data flowing *in*.
//!     - `ready` is the downstream's ready flowing *in* (= "is
//!       downstream ready for me to send next?").
//! - When used as widget **O** (`SynchronousIO::O`):
//!     - `data` is the widget's data flowing *out* to downstream.
//!     - `ready` is the widget's ready flowing *out* to upstream
//!       (= "am I ready to accept next from upstream?").
//!
//! The struct shape is identical for both directions; only the
//! semantic meaning of each field differs by role.  This matches the
//! existing [`super::StreamIO`] pattern.

use rhdl::prelude::*;

/// One item flowing through an [`RCStream`].
///
/// Pairs the payload `data: T` with the framing marker `frame: F`.
/// For streams without framing (`F = ()`), the `frame` field is the
/// unit value and adds no wire bits.
/// `Default` is derived (requiring `T: Default, F: Default`) so that
/// `Item<T, F>` can be the payload of widgets whose own `Default` is
/// derived through it — notably [`crate::fifo::asynchronous::AsyncFIFO`]
/// inside [`super::cdc::RCStreamCdc`].
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
pub struct Item<T: Digital, F: Digital> {
    /// Payload data.
    pub data: T,
    /// Framing marker.  Use `()` for streams without per-item framing.
    pub frame: F,
}

/// A typed, latency-insensitive stream of `T`-typed items, optionally
/// carrying framing markers of type `F`.
///
/// Pairs an `Option<Item<T, F>>` source-→sink data signal with a
/// `bool` sink-→source ready signal.  See module docs for the wire
/// encoding, framing patterns, and Synchronous-widget convention.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct RCStream<T: Digital, F: Digital> {
    /// Source → sink data signal.  `None` = idle (TVALID = 0);
    /// `Some(item)` = data this cycle (TVALID = 1).
    pub data: Option<Item<T, F>>,
    /// Sink → source ready signal.  `true` = ready to accept the
    /// next item.
    pub ready: bool,
}

/// A domain-typed [`RCStream`] — the same wire encoding, with both
/// signals carried as [`Signal`]s in clock domain `D`.
///
/// This is the bus type for widgets that live in the **multi-domain**
/// ([`Circuit`]) family but expose an `RCStream` port.  In the
/// single-domain ([`Synchronous`]) family the domain is implicit and
/// plain [`RCStream`] is the right type; here the domain is part of
/// the type, so connecting a `D = Red` port to a `D = Blue` port is a
/// compile error rather than a silent CDC bug.
///
/// # Role convention
///
/// Identical to [`RCStream`] — the struct shape is the same for both
/// directions and only the meaning of each field changes with the
/// role:
///
/// - As widget **I**: `data` is the upstream's data flowing *in*;
///   `ready` is the downstream's ready flowing *in*.
/// - As widget **O**: `data` is this widget's data flowing *out*;
///   `ready` is this widget's ready flowing *out* to the upstream.
///
/// # This type cannot express a clock-domain crossing
///
/// Both fields are in the *same* domain `D`, so `AsyncRCStream`
/// describes one **end** of a connection, not a crossing.  A crossing
/// widget's data-in and ready-in are in different domains by
/// definition, so it cannot use this type for its ports — see
/// [`super::cdc::RCStreamCdc`], whose `In`/`Out` name the two domains
/// separately.  Use `AsyncRCStream` for a single-domain widget that
/// participates in a multi-domain composition; use `RCStreamCdc` to
/// actually move items between domains.
#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
pub struct AsyncRCStream<T: Digital, F: Digital, D: Domain> {
    /// Source → sink data signal in domain `D`.  `None` = idle
    /// (TVALID = 0); `Some(item)` = data this cycle (TVALID = 1).
    pub data: Signal<Option<Item<T, F>>, D>,
    /// Sink → source ready signal in domain `D`.  `true` = ready to
    /// accept the next item.
    pub ready: Signal<bool, D>,
}

/// Construct an idle [`RCStream`] (`data: None`, `ready: <ready>`).
///
/// The kernel-callable form of "no item this cycle".  The ready
/// argument lets the caller still drive backpressure during an
/// idle cycle.
#[kernel]
pub fn idle<T: Digital, F: Digital>(ready: bool) -> RCStream<T, F> {
    RCStream::<T, F> { data: None, ready }
}

/// Construct an [`RCStream`] carrying `Some(item)` this cycle.
#[kernel]
pub fn send<T: Digital, F: Digital>(item: Item<T, F>, ready: bool) -> RCStream<T, F> {
    RCStream::<T, F> {
        data: Some(item),
        ready,
    }
}

/// Construct an [`Item`] from a payload + framing marker.
#[kernel]
pub fn item<T: Digital, F: Digital>(data: T, frame: F) -> Item<T, F> {
    Item::<T, F> { data, frame }
}

/// Convenience: construct an [`Item<T, ()>`] (no framing).
#[kernel]
pub fn item_unframed<T: Digital>(data: T) -> Item<T, ()> {
    Item::<T, ()> { data, frame: () }
}

/// Lift a domain-less [`RCStream`] into clock domain `D`.
///
/// The bridge from the single-domain ([`Synchronous`]) world to the
/// multi-domain ([`Circuit`]) world: a `Synchronous` widget's
/// `RCStream` port becomes an [`AsyncRCStream`] port when that widget
/// is placed in a multi-domain composition.  Pure re-wrapping — no
/// logic, no cost.
#[kernel]
pub fn lift<T: Digital, F: Digital, D: Domain>(s: RCStream<T, F>) -> AsyncRCStream<T, F, D> {
    AsyncRCStream::<T, F, D> {
        data: signal::<Option<Item<T, F>>, D>(s.data),
        ready: signal::<bool, D>(s.ready),
    }
}

/// Lower an [`AsyncRCStream`] in domain `D` to a domain-less
/// [`RCStream`].  The inverse of [`lift`].
#[kernel]
pub fn lower<T: Digital, F: Digital, D: Domain>(s: AsyncRCStream<T, F, D>) -> RCStream<T, F> {
    RCStream::<T, F> {
        data: s.data.val(),
        ready: s.ready.val(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Item<T, F>` is a plain `Digital` struct — should round-trip
    /// through pack/unpack with no surprises.
    #[test]
    fn item_default_roundtrip() {
        let i: Item<b8, bool> = Item {
            data: bits::<8>(0xab),
            frame: true,
        };
        assert_eq!(i.data.raw(), 0xab);
        assert_eq!(i.frame, true);
    }

    /// `RCStream<T, F>` with `data: None` is a valid idle cycle.
    #[test]
    fn rcstream_idle_construction() {
        let s: RCStream<b8, ()> = RCStream {
            data: None,
            ready: true,
        };
        assert!(s.data.is_none());
        assert_eq!(s.ready, true);
    }

    /// `RCStream<T, F>` with `data: Some(item)` carries the item.
    #[test]
    fn rcstream_send_construction() {
        let i: Item<b8, ()> = Item {
            data: bits::<8>(0x42),
            frame: (),
        };
        let s: RCStream<b8, ()> = RCStream {
            data: Some(i),
            ready: false,
        };
        match s.data {
            Some(item) => {
                assert_eq!(item.data.raw(), 0x42);
            }
            None => panic!("expected Some(item), got None"),
        }
        assert_eq!(s.ready, false);
    }

    /// `lift` then `lower` is the identity — the domain wrapper carries
    /// no information of its own, so the round trip must be lossless
    /// for both the idle and the carrying case.
    #[test]
    fn lift_lower_round_trip() {
        let carrying: RCStream<b8, bool> = RCStream {
            data: Some(Item {
                data: bits::<8>(0x7E),
                frame: true,
            }),
            ready: true,
        };
        let idle: RCStream<b8, bool> = RCStream {
            data: None,
            ready: false,
        };
        for original in [carrying, idle] {
            let lifted = lift::<b8, bool, Red>(original);
            let recovered = lower::<b8, bool, Red>(lifted);
            assert_eq!(recovered, original, "lift/lower must round-trip losslessly");
        }
    }

    /// The lifted signals land in the requested domain and carry the
    /// original values.
    #[test]
    fn lift_places_both_signals_in_the_domain() {
        let s: RCStream<b8, ()> = RCStream {
            data: Some(Item {
                data: bits::<8>(0x3C),
                frame: (),
            }),
            ready: true,
        };
        let lifted = lift::<b8, (), Blue>(s);
        assert!(lifted.ready.val());
        match lifted.data.val() {
            Some(it) => assert_eq!(it.data.raw(), 0x3C),
            None => panic!("expected the item to survive the lift"),
        }
    }

    /// Framing parameter F = bool gives end-of-frame marker
    /// (TLAST-equivalent).
    #[test]
    fn rcstream_with_eof_framing() {
        let last_item: Item<b8, bool> = Item {
            data: bits::<8>(0xFF),
            frame: true,
        };
        let middle: Item<b8, bool> = Item {
            data: bits::<8>(0x01),
            frame: false,
        };
        assert!(last_item.frame);
        assert!(!middle.frame);
    }
}
