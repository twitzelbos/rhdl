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
pub mod zip;

// Convenience re-exports so downstream code can `use
// rhdl_fpga::rcstream::{Item, RCStream, RCStreamRelay}` without
// spelling sub-module paths.
pub use bus::{AsyncRCStream, Item, RCStream};
pub use cdc::RCStreamCdc;
pub use chunked::RCStreamChunked;
pub use filter::RCStreamFilter;
pub use filter_map::RCStreamFilterMap;
pub use flatten::RCStreamFlatten;
pub use map::RCStreamMap;
pub use relay::RCStreamRelay;
pub use tee::RCStreamTee;
pub use zip::RCStreamZip;
