#![warn(missing_docs)]
//! Sample mixers — the complex multiply at the heart of both the
//! transmit modulator and the receive down-converter.
//!
//! One arithmetic, two uses: transmit multiplies an [`Iq`](super::iq::Iq)
//! carrier by a [`Real`](super::iq::Real) envelope; receive multiplies
//! real ADC samples by an `Iq` carrier. Multiplication commutes, so the
//! same widget serves both.
//!
//! # Why several widgets rather than one generic
//!
//! Knowing an operand is real is worth real silicon:
//!
//! | A | B | multiplies |
//! |---|---|---|
//! | `Iq` | `Iq` | **4** |
//! | `Iq` | `Real` | **2** |
//! | `Real` | `Real` | **1** |
//!
//! Two tidier-looking options were rejected, both for the same reason.
//! Representing a real operand as an `Iq` with `im` tied to zero, or
//! selecting with a `const IS_COMPLEX: bool` and an `if`, would each
//! leave the saving to a later optimisation pass — and CLAUDE.md §4 is
//! explicit that `if`/`else` lowers to a mux where **both branches
//! always evaluate**. The emitted netlist would still contain four
//! multiplies.
//!
//! **A resource claim that cannot be tested is not a resource claim.**
//! With separate widgets the multiplier count is structural, visible in
//! the Tier 3 snapshot, and asserted by `multiplier_count_is_as_claimed`
//! in each module.
//!
//! # Rounding
//!
//! Convergent (round-half-to-even), chosen by measurement rather than
//! by following the reference implementation. Narrowing the oscillator
//! to a 14-bit DAC, worst discrete spur against broadband floor:
//!
//! | rule | worst spur | floor | DC |
//! |---|---|---|---|
//! | truncate | −81.1 dBc | −138.3 | **−79.1** |
//! | round-half-up | −98.0 | −138.3 | −96.0 |
//! | **convergent** | **−103.0** | −137.3 | −102.2 |
//! | dither | −104.1 | **−125.3** | −102.2 |
//!
//! The usual argument for skipping convergent is that exact ties are
//! rare. That holds when many bits are discarded; here the drop is
//! small, so a tie is roughly **1 sample in 16**, and rounding all of
//! them the same way is a systematic error correlated with the signal —
//! a spur rather than noise. Dither buys 1.1 dB of spur for 13 dB of
//! floor, which is the wrong trade for a sensitivity-limited
//! instrument.
//!
//! # No saturation
//!
//! The full product is carried at its natural width, so the
//! maximum-negative-squared case (`−2^(n−1)` squared, which needs
//! `A+B` bits rather than `A+B−1`) cannot overflow. This matches the
//! AMD Complex Multiplier (PG104), whose natural width is "the sum of
//! the input widths plus one" and which has no saturation logic at all:
//! overflow at a narrowing stage is a consequence of the chosen output
//! width, not of the multiplier.
//!
//! # Clock domain
//!
//! Both inputs are necessarily in one domain and no mechanism is needed
//! to enforce it: a `Synchronous` widget receives a single implicit
//! `ClockReset`, so two domains cannot be expressed inside one. The
//! domain attaches at `Adapter<C, D>`, and crossing requires
//! `rcstream::cdc::RCStreamCdc` explicitly.

pub mod complex;
pub mod complex_real;
pub mod rounding;

pub use complex::ComplexMixer;
pub use complex_real::ComplexRealMixer;
