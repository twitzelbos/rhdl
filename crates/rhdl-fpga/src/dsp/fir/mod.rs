#![warn(missing_docs)]
//! Finite impulse response filters.
//!
//! One widget so far, [`symmetric::SymmetricFir`], which is the shape a
//! CIC compensator needs: linear phase, modest length, running at a
//! decimated rate. See [`crate::dsp::cic::compensator`] for the design
//! side — this module is the hardware that executes a tap set, not the
//! thing that chooses one.

pub mod symmetric;

pub use symmetric::SymmetricFir;

/// Accumulator width that cannot overflow for the given shape.
///
/// A product of a `w_in`-bit and a `w_coeff`-bit signed value needs
/// `w_in + w_coeff` bits, and summing `taps` of them needs
/// `ceil(log2(taps))` more. Folding adds one bit before the multiply,
/// because a symmetric pair is added first.
pub const fn accumulator_width(w_in: usize, w_coeff: usize, taps: usize) -> usize {
    let mut growth = 0;
    let mut v = 1;
    while v < taps {
        v *= 2;
        growth += 1;
    }
    // +1 for the fold's pre-add.
    w_in + w_coeff + growth + 1
}

/// Is `w_acc` wide enough for this shape?
pub const fn accumulator_width_is_sufficient(
    w_in: usize,
    w_coeff: usize,
    taps: usize,
    w_acc: usize,
) -> bool {
    w_acc >= accumulator_width(w_in, w_coeff, taps)
}
