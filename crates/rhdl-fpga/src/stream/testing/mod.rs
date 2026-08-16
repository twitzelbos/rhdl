//! Cores useful for testing streams.
//!
//! See [`sinks`] before writing a stream test.  The sink's readiness
//! policy decides what the test can find: a sink whose `ready` is
//! random and independent of the data stalls constantly and still
//! cannot catch a widget that forgets to consume its own rejects.
//! [`sinks::data_gated`] can.
#[doc(hidden)]
pub mod double;
pub mod lazy_random;
#[doc(hidden)]
pub mod single;
pub mod single_stage;
pub mod sink_from_fn;
pub mod sinks;
pub mod source_from_fn;
pub mod utils;
