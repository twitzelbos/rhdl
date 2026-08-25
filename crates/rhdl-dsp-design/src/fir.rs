#![warn(missing_docs)]
//! Width arithmetic for FIR filters.

/// Accumulator width that cannot overflow for the given shape.
///
/// A product of a `w_in`-bit and a `w_coeff`-bit signed value needs
/// `w_in + w_coeff` bits, and summing `taps` of them needs
/// `ceil(log2(taps))` more. The extra bit covers a folded filter's
/// pre-add, where a symmetric pair is summed before the multiply — one
/// bit wasted in the unfolded case, against two functions that could
/// drift apart.
pub const fn accumulator_width(w_in: usize, w_coeff: usize, taps: usize) -> usize {
    let mut growth = 0;
    let mut v = 1;
    while v < taps {
        v *= 2;
        growth += 1;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_is_ceil_log2_of_the_tap_count() {
        // +1 for the fold's pre-add throughout.
        assert_eq!(accumulator_width(8, 8, 1), 8 + 8 + 0 + 1);
        assert_eq!(accumulator_width(8, 8, 2), 8 + 8 + 1 + 1);
        assert_eq!(accumulator_width(8, 8, 3), 8 + 8 + 2 + 1);
        assert_eq!(accumulator_width(8, 8, 4), 8 + 8 + 2 + 1);
        assert_eq!(accumulator_width(8, 8, 5), 8 + 8 + 3 + 1);
    }

    #[test]
    fn sufficiency_agrees_with_the_width() {
        let w = accumulator_width(12, 14, 15);
        assert!(accumulator_width_is_sufficient(12, 14, 15, w));
        assert!(!accumulator_width_is_sufficient(12, 14, 15, w - 1));
    }
}
