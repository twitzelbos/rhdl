#![warn(missing_docs)]
//! AXI4-Stream interop for [`super::RCStream<T, F>`].
//!
//! Two translation widgets:
//!
//! - [`axi_to_rcstream::AxiToRCStream<T, F>`] — wraps an AXI4-Stream
//!   master input as an [`super::RCStream<T, F>`] source.
//! - [`rcstream_to_axi::RCStreamToAxi<T, F>`] — wraps an
//!   [`super::RCStream<T, F>`] source as an AXI4-Stream master output.
//!
//! Each translation widget includes a [`crate::lid::carloni::Carloni`]
//! skid-buffer on the AXI side to break combinatorial paths between
//! the AXI bus and the RCStream-side logic — same pattern as the
//! existing [`crate::axi4lite::stream`] widgets.
//!
//! # Signal mapping
//!
//! | RCStream side                    | AXI4-Stream side |
//! |---|---|
//! | `data: Option<Item<T, F>>::is_some()` | `TVALID` |
//! | `Item::data: T`                       | `TDATA`  |
//! | `Item::frame: F`                      | `TUSER`  |
//! | `ready: bool`                         | `TREADY` |
//!
//! **TLAST is NOT a separate signal in this interop.**  Users who
//! need TLAST-equivalent end-of-frame markers should encode them in
//! `F` (e.g., `F = bool`, where TUSER becomes a 1-bit signal carrying
//! the end-of-frame marker; AXI4-Stream consumers wire that 1-bit
//! TUSER bit to their TLAST input).  Adding a separate TLAST signal
//! is straightforward in a follow-up PR but adds a typing question
//! (which bit of `F` is "last"?) that's better answered case-by-case
//! than baked into the bus translator.
//!
//! # Relationship to `axi4lite::stream::*`
//!
//! These widgets are **parallel to and independent of** the existing
//! [`crate::axi4lite::stream::axi_to_rhdl::Axi2Rhdl`] and
//! [`crate::axi4lite::stream::rhdl_to_axi::Rhdl2Axi`] widgets.  Those
//! widgets translate AXI4-Stream ↔ [`crate::stream::StreamIO`] (the
//! existing bus type, no framing parameter).  The widgets here
//! translate AXI4-Stream ↔ [`super::RCStream<T, F>`] (the typed bus
//! with the framing parameter).  Both interop paths coexist; users
//! pick based on which bus type their design uses.
//!
//! # Round-trip property
//!
//! `axi → AxiToRCStream → RCStream<T, F> → RCStreamToAxi → axi` must
//! produce a byte-identical waveform on the AXI side (modulo the
//! one-cycle Carloni latency on each side).  This is the validation
//! criterion — see the round-trip test in
//! [`rcstream_to_axi::tests::test_round_trip`].
//!
//! See `stream-bus-architecture.md` §10 for the full design.

pub mod axi_to_rcstream;
pub mod rcstream_to_axi;

// Convenience re-exports.
pub use axi_to_rcstream::AxiToRCStream;
pub use rcstream_to_axi::RCStreamToAxi;
