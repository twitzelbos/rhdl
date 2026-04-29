//! Clock domain crossing cores
#![warn(missing_docs)]
/// A Clock domain crossing binary counter
pub mod cross_counter;
/// A multi-bit handshake bridge for slow CDC of arbitrary `T: Digital`
pub mod slow_crosser;
/// A one-bit synchronizer
pub mod synchronizer;
/// An N-stage one-bit synchronizer chain
pub mod synchronizer_chain;
