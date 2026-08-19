#![warn(missing_docs)]
//! Small `RCStream` utilities: a constant source, and the split and
//! combine pair that makes the [`Iq`](crate::dsp::iq::Iq) /
//! [`Real`](crate::dsp::iq::Real) / [`Imag`](crate::dsp::iq::Imag) type
//! algebra usable in a real chain.
//!
//! Without split and combine the sample types are decorative: routing a
//! complex stream into a widget that wants a real one is not expressible
//! at all, so the `Real × Iq` instantiation of a mixer could never be
//! reached from an `Iq` source.

pub mod combine;
pub mod constant;
pub mod split;

pub use combine::IqCombine;
pub use constant::RCStreamConstant;
pub use split::IqSplit;
