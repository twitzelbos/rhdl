//! DSP Related Cores

use rhdl::prelude::*;

pub mod cic;
pub mod cordic;
pub mod ddc;
pub mod iq;
pub mod lerp;
pub mod mixer;
pub mod nco;
pub mod rx_trigger;
pub mod sync;

/// Widen a signed value, sign-extending explicitly.
///
/// # Why this exists instead of `SignedBits::resize`
///
/// `resize` is the natural spelling and is **wrong for a value
/// unwrapped from an `Option`**: it emits zero extension there, while
/// the Rust simulator sign-extends. Tiers 1 and 2 therefore pass and
/// only the `iverilog` round-trip fails — or, as here, `descriptor()`
/// rejects a later negation with "cannot negate unsigned value",
/// because the widened value has lost its signedness.
///
/// The same operation on a *direct* input emits `$signed({{n{v[msb]}},
/// v})` correctly, so RHDL can do it; something about extraction from
/// an aggregate drops the signedness. This is the third place in the
/// tree to hit it (see also `dsp::nco::modulation` and
/// `tests/signed_literal_comparison.rs`), and it is filed as compiler
/// work.
///
/// Zero-extending and then correcting uses only bit operations and
/// addition, neither of which depends on the operand's declared
/// signedness, so it is correct either way.
#[kernel]
#[doc(hidden)]
pub fn sign_extend<const FROM: usize, const TO: usize>(v: SignedBits<FROM>) -> SignedBits<TO>
where
    rhdl::bits::W<FROM>: BitWidth,
    rhdl::bits::W<TO>: BitWidth,
{
    let raw = v.as_unsigned();
    let negative = (raw & bits::<FROM>(1 << (FROM - 1))) != bits::<FROM>(0);
    let widened = raw.resize::<TO>();
    let fill = if negative {
        bits::<TO>((1 << TO) - (1 << FROM))
    } else {
        bits::<TO>(0)
    };
    (widened + fill).as_signed()
}
