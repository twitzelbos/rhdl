#![warn(missing_docs)]
//! `rcstream` — RHDL's canonical typed, latency-insensitive streaming
//! bus.
//!
//! `RCStream<T, F>` carries a payload of type `T` and an optional
//! framing marker of type `F`, paired with a `bool` ready signal in
//! the opposite direction.  The "RC" prefix names the two design
//! properties the bus inherits: **R**HDL's type system and
//! **C**arloni's latency-insensitive-design (LID) theorem.
//!
//! # Module layout
//!
//! - [`bus`] — the [`bus::Item`] and [`bus::RCStream`] types + kernel-
//!   callable construction helpers.
//! - [`relay`] — [`relay::RCStreamRelay`], a Carloni skid-buffer with
//!   the typed `RCStream` interface.  The canonical pipeline-stage
//!   primitive for `RCStream` connections.
//! - [`cdc`] — [`cdc::RCStreamCdc`], a clock-domain crossing for an
//!   `RCStream` connection, plus [`bus::AsyncRCStream`], the
//!   domain-typed form of the bus for multi-domain compositions.
//! - [`fanout`] — [`fanout::RCStreamFanout`], a **broadcast** to `N`
//!   sinks: every branch receives every item, held until the slowest
//!   has taken it.  Distinct from [`tee`], which *splits* a tuple
//!   stream so each branch sees a different projection.
//!
//! # What each widget does to the framing
//!
//! Every widget here is generic over `F: Digital` and none constrains
//! it — there is no `Framing` trait, and none is needed: no widget
//! requires an operation on the marker beyond moving it, pairing it, or
//! comparing it, and comparison comes free with `Digital`. So "which
//! framings are supported" has one answer everywhere (any `Digital`
//! type) and is not a useful way to tell these widgets apart.
//!
//! What differs is what each does *to* the framing:
//!
//! | widget | `F` in | `F` out |
//! |---|---|---|
//! | [`relay`], [`filter`], [`map`], [`filter_map`], [`fanout`], [`cdc`] | `F` | `F` |
//! | [`credit`]`::*` | `F` | `F` |
//! | [`axi_stream`]`::*` | `F` ↔ `TUSER` | — |
//! | [`chunked`] | `F` | `[F; N]` — every element's marker, positionally |
//! | [`flatten`] | `F` | `(F, bool)` — the group's marker, plus last-of-group |
//! | [`zip`] | `F`, `G` | `(F, G)` — both, unsynchronised |
//! | [`tee`] | `F`, `G` | `F` and `G`, one per branch |
//! | [`util::split`] | `F` | `F` on **both** halves |
//! | [`util::combine`] | `F`, `F` | `F`, and `frame_mismatch` if they disagree |
//!
//! **The composition is the part worth writing down**, because it is
//! spread over two files otherwise. Chunking and then flattening does
//! *not* return you to `F`:
//!
//! ```text
//!   RCStream<T, F>
//!     --chunked-->  RCStream<[T; N], [F; N]>
//!     --flatten-->  RCStream<T, ([F; N], bool)>
//! ```
//!
//! The payload round-trips; the framing accumulates. `tests` pins that
//! composition so neither widget's rule can change without the other
//! being reconsidered.
//!
//! Two traits — `ChunkFraming<N>` and `FlattenFraming`, each with an
//! associated type naming the rule — were designed and **rejected**.
//! Each would have had exactly one blanket impl, so it would state no
//! fact the type system does not already enforce: `type O =
//! RCStream<[T; N], [F; N]>` *is* the rule, checked at every connection.
//! And the composed spelling would have been
//! `<<F as ChunkFraming<N>>::Chunked as FlattenFraming>::Flattened`,
//! which is strictly harder to read than `([F; N], bool)`. What was
//! actually missing was *discoverability*, not checking — so this table
//! is the fix, and it lives here because this is where someone chaining
//! widgets looks.
//!
//! # Relationship to the existing `stream` module
//!
//! `rcstream` lives in parallel to [`crate::stream`] (the existing
//! `StreamIO<T, S>`-based widget library).  The two are independent;
//! existing `stream::*` widgets are NOT being migrated.  `rcstream`
//! is the bus type that **new** widgets can opt into when the typed-
//! framing-marker / typed-clock-domain / LID-correct properties
//! matter.
//!
//! # Design plan
//!
//! See `stream-bus-architecture.md` at the repository root for the
//! full design rationale + AXI4-Stream comparison + framing-pattern
//! catalogue.

pub mod axi_stream;
pub mod bus;
pub mod cdc;
pub mod chunked;
pub mod credit;
pub mod fanout;
pub mod filter;
pub mod filter_map;
pub mod flatten;
pub mod map;
pub mod relay;
pub mod tee;
pub mod testing;
pub mod util;
pub mod zip;

// Convenience re-exports so downstream code can `use
// rhdl_fpga::rcstream::{Item, RCStream, RCStreamRelay}` without
// spelling sub-module paths.
pub use bus::{AsyncRCStream, Item, RCStream};
pub use cdc::RCStreamCdc;
pub use chunked::RCStreamChunked;
// `fanout` is a flat module exactly like `filter` and `map`, and was the
// only one of the nine missing from this list -- reachable only as
// `rcstream::fanout::RCStreamFanout` despite having an example and the
// full test stack. The `credit`, `util` and `axi_stream` widgets are
// deliberately absent instead: they are grouped sub-modules that
// re-export at their own level.
pub use fanout::RCStreamFanout;
pub use filter::RCStreamFilter;
pub use filter_map::RCStreamFilterMap;
pub use flatten::RCStreamFlatten;
pub use map::RCStreamMap;
pub use relay::RCStreamRelay;
pub use tee::RCStreamTee;
pub use zip::RCStreamZip;

#[cfg(test)]
mod tests {
    /// **Every flat module's widget is re-exported at this level.**
    ///
    /// `RCStreamFanout` was missing from the list above for as long as it
    /// existed: reachable only as `rcstream::fanout::RCStreamFanout`
    /// while its eight siblings were one path shorter. Nothing failed,
    /// because nothing named the set.
    ///
    /// This test names it. Each line is a `use` of the short path, so a
    /// widget added to a flat module without a matching `pub use` fails
    /// to compile here rather than being quietly harder to reach.
    ///
    /// The grouped sub-modules — `credit`, `util`, `axi_stream` — are
    /// deliberately *not* included: they re-export at their own level,
    /// which keeps `rcstream::` from becoming a flat namespace of
    /// everything.
    /// **Chunking then flattening does not return the framing to `F`.**
    ///
    /// The payload round-trips and the framing accumulates:
    /// `F -> [F; N] -> ([F; N], bool)`. Written as a type equality so
    /// that changing either widget's framing rule breaks here, which is
    /// the only place the two rules are considered together.
    #[test]
    fn chunk_then_flatten_accumulates_framing() {
        use super::{RCStream, RCStreamChunked, RCStreamFlatten};
        use rhdl::prelude::*;

        const N: usize = 4;
        type F = bool;

        // The chunker's output is the flattener's input. If these two
        // associated types stop agreeing, this stops compiling.
        type ChunkOut = <RCStreamChunked<b8, F, 3, N> as SynchronousIO>::O;
        type FlattenIn = <RCStreamFlatten<b8, [F; N], 3, N> as SynchronousIO>::I;
        fn connect(x: ChunkOut) -> FlattenIn {
            x
        }

        // And the far end is *not* `RCStream<b8, F>`.
        type FlattenOut = <RCStreamFlatten<b8, [F; N], 3, N> as SynchronousIO>::O;
        fn accumulated(x: FlattenOut) -> RCStream<b8, ([F; N], bool)> {
            x
        }

        let mid = RCStream::<[b8; N], [F; N]> {
            data: None,
            ready: true,
        };
        let end = RCStream::<b8, ([F; N], bool)> {
            data: None,
            ready: true,
        };
        assert_eq!(connect(mid), mid);
        assert_eq!(accumulated(end), end);
    }

    #[test]
    fn every_flat_widget_is_re_exported() {
        #[allow(unused_imports)]
        use super::{
            AsyncRCStream, Item, RCStream, RCStreamCdc, RCStreamChunked, RCStreamFanout,
            RCStreamFilter, RCStreamFilterMap, RCStreamFlatten, RCStreamMap, RCStreamRelay,
            RCStreamTee, RCStreamZip,
        };

        // And the grouped ones are reachable at their own level.
        #[allow(unused_imports)]
        use super::{
            axi_stream::{AxiToRCStream, RCStreamToAxi},
            credit::{CreditMux, CreditRCStreamRelay, CreditSink, CreditSource},
            util::{IqCombine, IqSplit, RCStreamConstant},
        };
    }
}
