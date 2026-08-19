#![warn(missing_docs)]
//! `Iq<W>` — a complex baseband sample.
//!
//! Everything downstream of the synthesizer — modulator, digital down
//! converter, packetizer — treats the oscillator's two outputs as one
//! complex number, not as two coincidental signals. Naming that makes
//! the pairing part of the type, so a widget cannot be handed a sine
//! and someone else's cosine.
//!
//! # Why `re`/`im` and not `i`/`q`
//!
//! RHDL kernels bind `i` for the input bundle and `q` for the state
//! bundle, universally. Fields named `i` and `q` would produce
//! expressions like `i.iq.i` and `q.amp.q`, where the meaning of each
//! letter depends on its position. `re`/`im` is unambiguous in that
//! context and is the standard mathematical spelling.
//!
//! The mapping to radio convention is fixed and documented on the
//! fields: **`re` is in-phase (I, cosine), `im` is quadrature (Q,
//! sine)**.

use rhdl::prelude::*;

/// A complex baseband sample, `W` bits per component.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
pub struct Iq<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// In-phase component — the **I** of I/Q, the cosine.
    pub re: SignedBits<W>,
    /// Quadrature component — the **Q** of I/Q, the sine.
    pub im: SignedBits<W>,
}

/// A purely **real** sample.
///
/// A newtype rather than a bare `SignedBits<W>`, so that "a real
/// signal" and "one component of a complex signal" are different types.
/// Without the distinction a mixer cannot select its arithmetic from
/// its inputs, and the multiplier count stops being visible in the
/// instantiation.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
pub struct Real<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The value.
    pub v: SignedBits<W>,
}

/// A purely **imaginary** sample.
///
/// The counterpart to [`Real`], and it closes the algebra: with all
/// three sample types the *result* type of a multiply follows from the
/// operand types.
///
/// | A | B | result | multiplies |
/// |---|---|---|---|
/// | [`Iq`] | [`Iq`] | [`Iq`] | 4 |
/// | [`Iq`] | [`Real`] or [`Imag`] | [`Iq`] | 2 |
/// | [`Real`] | [`Real`] | [`Real`] | 1 |
/// | [`Imag`] | [`Imag`] | [`Real`] **negated** | 1 |
/// | [`Real`] | [`Imag`] | [`Imag`] | 1 |
///
/// The `Imag × Imag → Real` row carries a sign flip, because
/// `i · i = −1`. Having it change the *type* is what makes the
/// negation explicit rather than a sign error waiting to happen.
#[derive(PartialEq, Clone, Copy, Debug, Digital, Default)]
pub struct Imag<const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The coefficient of `i`.
    pub v: SignedBits<W>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pair costs exactly two components and nothing else.
    ///
    /// Naming a type is only free if it stays free; a padded
    /// representation would silently widen every bus it travels on.
    #[test]
    fn iq_costs_exactly_two_components() {
        assert_eq!(<Iq<18> as Digital>::BITS, 36);
        assert_eq!(<Iq<14> as Digital>::BITS, 28);
        assert_eq!(
            <Iq<18> as Digital>::BITS,
            2 * <SignedBits<18> as Digital>::BITS
        );
    }

    /// The scalar types cost exactly their component and nothing else.
    #[test]
    fn scalar_types_are_free() {
        assert_eq!(<Real<18> as Digital>::BITS, 18);
        assert_eq!(<Imag<18> as Digital>::BITS, 18);
    }

    /// Default is the origin, which is the correct idle sample for a
    /// transmit chain: zero amplitude, not an arbitrary phase.
    #[test]
    fn default_is_the_origin() {
        let z = Iq::<18>::default();
        assert_eq!(z.re.raw(), 0);
        assert_eq!(z.im.raw(), 0);
    }
}
