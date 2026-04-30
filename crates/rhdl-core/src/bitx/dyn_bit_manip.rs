//! Module for dynamic bit manipulation utilities.
//!
//! This module provides functions for converting between `BitX` vectors and
//! `BigInt`/`BigUint`, as well as basic bitwise operations on `BitX` vectors.
//! It also includes utility functions for shifting, adding, and manipulating
//! `BitX` vectors.
//!
use num_bigint::BigUint;
use num_bigint::{BigInt, Sign};
use std::iter::repeat;

use crate::bitx::{BitX, bitx_vec};

/// Convert a vector of `BitX` to a `BigInt`, interpreting the bits as a signed integer
/// in two's complement representation. Returns `None` if any bit is `X`.
pub fn to_bigint(bits: &[BitX]) -> Option<BigInt> {
    let bits = bits
        .iter()
        .map(|x| x.to_bool())
        .collect::<Option<Vec<_>>>()?;
    Some(if bits.last() != Some(&true) {
        let bits = bits
            .iter()
            .map(|x| if *x { 1 } else { 0 })
            .collect::<Vec<_>>();
        BigInt::from_radix_le(Sign::Plus, &bits, 2).unwrap()
    } else {
        let bits = bits
            .iter()
            .map(|x| if *x { 0 } else { 1 })
            .collect::<Vec<_>>();
        -(BigInt::from_radix_le(Sign::Plus, &bits, 2).unwrap() + 1_i32)
    })
}

/// Convert a `BigInt` to a vector of `BitX` with the specified length,
/// interpreting the integer in two's complement representation.
pub fn from_bigint(bi: &BigInt, len: usize) -> Box<[BitX]> {
    if bi < &BigInt::ZERO {
        let bi = -bi - 1_i32;
        let bits = from_bigint(&bi, len);
        bits.into_iter().map(|x| !x).collect()
    } else {
        bitx_vec(&(0..len as u64).map(|pos| bi.bit(pos)).collect::<Vec<_>>())
    }
}

/// Convert a vector of `BitX` to a `BigUint`. Returns `None` if any bit is `X`.
pub fn to_biguint(bits: &[BitX]) -> Option<BigUint> {
    let bits = bits
        .iter()
        .map(|x| x.to_bool())
        .collect::<Option<Vec<_>>>()?;
    let bits = bits
        .iter()
        .map(|x| if *x { 1 } else { 0 })
        .collect::<Vec<_>>();
    Some(BigUint::from_radix_le(&bits, 2).unwrap())
}

/// Convert a `BigUint` to a vector of `BitX` with the specified length.
pub fn from_biguint(bi: &BigUint, len: usize) -> Vec<BitX> {
    (0..len as u64).map(|pos| bi.bit(pos).into()).collect()
}

pub(crate) fn add_one(a: &[BitX]) -> Vec<BitX> {
    a.iter()
        .scan(BitX::One, |carry, b| {
            let sum = *b ^ *carry;
            *carry &= *b;
            Some(sum)
        })
        .collect()
}

pub(crate) fn full_add(a: &[BitX], b: &[BitX]) -> Vec<BitX> {
    a.iter()
        .zip(b.iter())
        .scan(BitX::Zero, |carry, (a, b)| {
            let sum = *a ^ *b ^ *carry;
            let new_carry = (*a & *b) | (*a & *carry) | (*b & *carry);
            *carry = new_carry;
            Some(sum)
        })
        .collect()
}

pub(crate) fn bit_not(a: &[BitX]) -> Vec<BitX> {
    a.iter().map(|b| !*b).collect()
}

pub(crate) fn bit_neg(a: &[BitX]) -> Vec<BitX> {
    add_one(&bit_not(a))
}

pub(crate) fn full_sub(a: &[BitX], b: &[BitX]) -> Vec<BitX> {
    full_add(a, &bit_neg(b))
}

pub(crate) fn bits_xor(a: &[BitX], b: &[BitX]) -> Vec<BitX> {
    a.iter().zip(b.iter()).map(|(a, b)| *a ^ *b).collect()
}

pub(crate) fn bits_and(a: &[BitX], b: &[BitX]) -> Vec<BitX> {
    a.iter().zip(b.iter()).map(|(a, b)| *a & *b).collect()
}

pub(crate) fn bits_or(a: &[BitX], b: &[BitX]) -> Vec<BitX> {
    a.iter().zip(b.iter()).map(|(a, b)| *a | *b).collect()
}

pub(crate) fn bits_shl(a: &[BitX], b: i64) -> Vec<BitX> {
    std::iter::repeat_n(BitX::Zero, b as usize)
        .chain(a.iter().copied())
        .take(a.len())
        .collect()
}

pub(crate) fn bits_shr(a: &[BitX], b: i64) -> Vec<BitX> {
    a.iter()
        .copied()
        .skip(b as usize)
        .chain(std::iter::repeat_n(BitX::Zero, b as usize))
        .take(a.len())
        .collect()
}

pub(crate) fn bits_shr_signed(a: &[BitX], b: i64) -> Vec<BitX> {
    let sign = a.last().copied().unwrap_or(BitX::Zero);
    a.iter()
        .copied()
        .skip(b as usize)
        .chain(repeat(sign))
        .take(a.len())
        .collect()
}

/// Move the first `n` bits of the array `a` to the most significant bits (end) of a new vector.
pub fn move_nbits_to_msb<T: Copy>(a: &[T], n: usize) -> Vec<T> {
    let (left, right) = a.split_at(n);
    [right, left].concat()
}

/// Pairwise max of two `usize` values, evaluable in `const`
/// context.  The companion to [`const_max!`] — used internally
/// to keep the macro expansion linear in argument count.
pub const fn const_max_pair(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

#[macro_export]
/// Macro to compute the maximum of a list of constant expressions at compile time.
///
/// Implementation note: the macro must NOT recurse with the
/// recursive call appearing more than once on the right-hand
/// side, or expansion is exponential in argument count.  An
/// earlier version expanded as `if $x > const_max!(rest) { $x }
/// else { const_max!(rest) }` — duplicating the recursive call
/// — which produced 2^(N-1) leaf occurrences for N arguments.
/// On a 22-variant `#[derive(Digital)]` enum this generated
/// over 3 million `0_usize` tokens for the `BITS` constant and
/// crashed rustc with SIGKILL after ~7 GB of RSS.  The current
/// expansion uses a const-fn helper so each level adds one
/// `const_max_pair` call — total tokens linear in argument
/// count.
macro_rules! const_max {
    ($x: expr) => ($x);
    ($x: expr, $($z: expr), +) => (
        $crate::bitx::dyn_bit_manip::const_max_pair($x, $crate::const_max!($($z), +))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concat_test() {
        let a = vec![true, false, true];
        let b = [a.as_slice()].concat();
        assert_eq!(b, vec![true, false, true]);
    }

    #[test]
    fn test_const_max_macro() {
        assert_eq!(const_max!(1, 2, 3, 4, 5), 5);
        assert_eq!(const_max!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10), 10);
    }

    /// Regression test for the OOM documented in
    /// `notes/kernel-macro-oom-resolved.md`.  This call passes
    /// 32 arguments to `const_max!`.  Under the prior
    /// duplicate-recursive-call form of the macro, the
    /// expansion would have produced 2^31 ≈ 2.1 billion leaf
    /// occurrences and crashed rustc.  Under the linear form,
    /// it expands to 31 nested `const_max_pair` calls and
    /// compiles in microseconds.  If this test ever stops
    /// compiling cleanly, the macro has regressed back to
    /// quadratic-or-worse expansion.
    #[test]
    fn test_const_max_does_not_explode_at_32_args() {
        let r = const_max!(
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32
        );
        assert_eq!(r, 32);
    }

    #[test]
    fn test_move_nbits_to_msb() {
        let a: Vec<bool> = (0..200).map(|_| rand::random()).collect();
        for n in 0..a.len() {
            let b = move_nbits_to_msb(&a, n);
            let c = a.iter().skip(n).chain(a.iter().take(n));
            assert!(c.eq(b.iter()));
        }
    }

    #[test]
    fn test_bigint_conversion() {
        let bits = bitx_vec(&[true, false, true, false]); // 5
        let bi = to_bigint(&bits).unwrap();
        assert_eq!(bi, BigInt::from(5));
        let bits_regen = from_bigint(&bi, 4);
        assert_eq!(bits_regen, bits);
        let bits = bitx_vec(&[true, true, false, true]); // -5
        let bi = to_bigint(&bits).unwrap();
        assert_eq!(bi, BigInt::from(-5));
        let bits_regen = from_bigint(&bi, 4);
        assert_eq!(bits_regen, bits);
    }

    #[test]
    fn test_bigint_extend_behavior() {
        let bits = bitx_vec(&[true, false, true, false]); // 5
        let bi = to_bigint(&bits).unwrap();
        let bits_regen = from_bigint(&bi, 8);
        assert_eq!(
            bits_regen,
            bitx_vec(&[true, false, true, false, false, false, false, false])
        );
    }
}
