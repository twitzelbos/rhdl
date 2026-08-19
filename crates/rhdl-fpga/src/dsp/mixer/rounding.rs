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
