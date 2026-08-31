#![warn(missing_docs)]
//! `SyncMark` — the framing marker that anchors a timing relationship.
//!
//! A `SyncMark`-framed [`RCStream`](crate::rcstream::bus::RCStream) carries
//! one bit alongside each sample answering a single question: *is this
//! the sample the timing contract is about?* On a receive path that is
//! the first sample of an acquisition; on the synthesizer's output it
//! is the first sample affected by a configuration change.
//!
//! # Why a newtype and not `bool`
//!
//! `F = bool` already means TLAST in this tree — end-of-frame — and is
//! used that way by nine widgets. A sync marker is the opposite end of
//! a frame and an unrelated contract. Making both `bool` would let a
//! packetizer's end-of-frame stream connect to a mixer's sync port and
//! typecheck.
//!
//! That is precisely the failure `rcstream::bus` exists to prevent:
//!
//! > The framing semantics are part of the type — mismatched `F`
//! > between two ends of a connection is a compile error, not a silent
//! > TUSER mismatch.
//!
//! The cost of the distinction is zero. `SyncMark` is one bit, exactly as
//! `bool` would be; the newtype exists only in the type system.
//!
//! It is `SyncMark` rather than `Sync` because `Sync` is in the Rust
//! prelude as [`std::marker::Sync`]. A type by that name in a module
//! that does `use rhdl::prelude::*` shadows the auto-trait, which is
//! the kind of thing that compiles until the day a derive expands to a
//! `Sync` bound.
//!
//! # The alignment contract
//!
//! Two `SyncMark`-framed streams that are supposed to describe the same
//! instant must assert [`SyncMark::sync`] on the *same cycle* where they
//! meet. A widget consuming both is entitled to treat a one-sided
//! assertion as an error rather than silently picking one — see
//! [`ComplexMixer`](crate::dsp::mixer::complex::ComplexMixer)'s
//! `frame_mismatch`, which reports exactly that.
//!
//! ## Across a clock domain
//!
//! The contract is about a *cycle*, and a cycle is a domain-local notion,
//! so crossing needs a word. A
//! [`RCStreamCdc`](crate::rcstream::cdc::RCStreamCdc) carries the marker
//! atomically with its payload — one FIFO over the whole `Item` — so
//! *which item* is marked always survives. Whether the *cycle* survives
//! depends on the drainage, not on the crossing: two crossings behind one
//! consumer stay in lockstep, two behind different consumers or different
//! backpressure do not.
//!
//! **So cross a pair of aligned streams as one `Item`, not as two
//! streams.** Combine first, cross once, split after. `rcstream::cdc`'s
//! module docs carry the measurement and the concrete right-and-wrong
//! shapes.
//!
//! This is what makes latency compensation checkable instead of merely
//! documented. The scheduler issues a configuration change early by the
//! self-reported latency; if it got the lead time wrong, the two
//! markers land on different cycles and the hardware says so.

use rhdl::prelude::*;

/// A framing marker naming one sample as the anchor of a timing
/// relationship.
///
/// See the module docs for why this is a newtype rather than `bool`,
/// and for the alignment contract two `SyncMark`-framed streams owe each
/// other.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
pub struct SyncMark {
    /// `true` on exactly the sample this marker refers to.
    ///
    /// A stream that never asserts it is well-formed — that is an
    /// un-anchored stream, not an error. What is an error is two
    /// streams that are supposed to be aligned asserting it on
    /// different cycles.
    pub sync: bool,
}

/// The un-marked value — an ordinary sample.
///
/// Kernel-callable, so a widget that framed nothing this cycle can say
/// so without spelling out the struct literal.
#[kernel]
pub fn clear() -> SyncMark {
    SyncMark { sync: false }
}

/// The marked value — the anchor sample.
#[kernel]
pub fn mark() -> SyncMark {
    SyncMark { sync: true }
}

/// `Sync` carrying `flag`.
///
/// The common case inside a kernel, where the marker is computed rather
/// than chosen between two constants.
#[kernel]
pub fn when(flag: bool) -> SyncMark {
    SyncMark { sync: flag }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One bit, so the newtype is free.
    #[test]
    fn sync_costs_exactly_one_bit() {
        assert_eq!(SyncMark::BITS, 1);
        assert_eq!(SyncMark::BITS, bool::BITS);
    }

    /// The default is un-marked. Load-bearing: `Item<T, SyncMark>` is
    /// `Default`-constructed inside reset paths and delay lines, and a
    /// default that asserted `sync` would inject a spurious anchor on
    /// every reset.
    #[test]
    fn the_default_is_unmarked() {
        assert!(!SyncMark::default().sync);
        assert_eq!(SyncMark::default(), clear());
    }

    /// The two constants are distinct and `when` agrees with both.
    #[test]
    fn the_constructors_agree() {
        assert_ne!(clear(), mark());
        assert_eq!(when(false), clear());
        assert_eq!(when(true), mark());
    }
}
