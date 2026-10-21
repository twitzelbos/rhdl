#![warn(missing_docs)]
//! Full AXI4 — five channels, bursts, transaction IDs.
//!
//! `axi4-architecture.md` is the plan and the record of why this is
//! shaped as it is. The short version of the two decisions a reader
//! will wonder about:
//!
//! # Why this is not `axi4lite` with extra fields
//!
//! [`crate::axi4lite`] stays, unchanged, and the two coexist.
//! `widget-roadmap.md` #56 proposed that AXI4-Lite become a degenerate
//! `Axi<32, 32, 0>`, and the existing module cannot become that: its
//! `AxilData`, `AxilAddr` and `AxilStrobe` are hard-coded `Bits<32>`
//! and `Bits<4>`, used 105 times across 18 files with ten more outside
//! the module. Unifying is a width-parameterisation refactor of ~28
//! files, and it buys a user nothing — a design on AXI4-Lite benefits
//! from Lite continuing to work, not from how it is spelled.
//!
//! There is also a reason of substance, not just cost. **AXI4-Lite has
//! no burst**, so its endpoints need no beat counter and its
//! interconnect no ID tracking. Expressing Lite through the AXI4 types
//! means every Lite endpoint carries fields it must assert are
//! constant — a conformance burden pointing the wrong way.
//!
//! `axi4::LiteBridge` (Phase 6) is the composition anyone actually
//! wants: an AXI4 master talking to a Lite endpoint.
//!
//! # Why 128 bits is the widest data bus
//!
//! [`AXI4_MAX_DATA_BITS`]. `Bits<N>` exists for `N` in `1..=128`, so a
//! 256- or 512-bit bus would need an array-of-words payload or a
//! kernel-language change. Neither is foreclosed; both are more than
//! this needs. `AxSIZE` tops out at 4 for a 16-byte bus, which is
//! within the 3-bit field the spec gives it.

pub mod types;
