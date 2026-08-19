#![warn(missing_docs)]
//! Convergent narrowing, shared by the mixers.

use rhdl::prelude::*;

/// Narrow by `DROP` bits with **convergent** (round-half-to-even)
/// rounding.
///
/// Ties — where the discarded bits are exactly one half — go to the
/// even result rather than always upward. With a small `DROP` a tie is
/// common (one sample in `2^DROP`), and rounding all of them the same
/// direction is a systematic error correlated with the signal, which
/// appears as a spur rather than as noise. See the module docs for the
/// measured difference.
///
/// # Two preconditions on the widths
///
/// Neither is reachable at any current instantiation, and both are one
/// width change away, so they are stated rather than left implicit.
///
/// 1. **`DROP >= 1`.** `1 << (DROP - 1)` underflows at `DROP == 0`, i.e.
///    a no-narrowing instantiation. There is nothing to round in that
///    case, so the right response is not to instantiate this.
/// 2. **`v + half` must not overflow `PROD_W`.** The module docs explain
///    why the *product* cannot overflow its natural width; this adds half
///    an LSB on top of it, and relies on the product not using the full
///    width. It holds comfortably at both mixers — `ComplexRealMixer`
///    carries `A_W + B_W` for a product bounded by `2^(A_W+B_W-2)`, one
///    spare bit, and `ComplexMixer` carries one more than that.
#[kernel]
#[doc(hidden)]
pub fn convergent<const PROD_W: usize, const OUT_W: usize, const DROP: usize>(
    v: SignedBits<PROD_W>,
) -> SignedBits<OUT_W>
where
    rhdl::bits::W<PROD_W>: BitWidth,
    rhdl::bits::W<OUT_W>: BitWidth,
{
    let half = bits::<PROD_W>(1 << (DROP - 1));
    let mask = bits::<PROD_W>((1 << DROP) - 1);
    let lsbs = v.as_unsigned() & mask;

    let rounded = (v + half.as_signed()) >> bits::<8>(DROP as u128);

    // A tie is exactly half; steer it to even.
    let tie = lsbs == half;
    let odd = (rounded.as_unsigned() & bits::<PROD_W>(1)) != bits::<PROD_W>(0);
    let adjusted = if tie && odd {
        rounded - signed::<PROD_W>(1)
    } else {
        rounded
    };
    adjusted.resize::<OUT_W>()
}
