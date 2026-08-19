use crate::{RHDLError, TypedBits};

use super::spec::{AluBinary, AluUnary};

pub fn unary(op: AluUnary, arg1: TypedBits) -> Result<TypedBits, RHDLError> {
    let op = op.into();
    crate::rhif::runtime_ops::unary(op, arg1)
}

pub fn binary(op: AluBinary, arg1: TypedBits, arg2: TypedBits) -> Result<TypedBits, RHDLError> {
    let op = op.into();
    crate::rhif::runtime_ops::binary(op, arg1, arg2)
}

/// Evaluate a binary operation whose operands may be **narrower than the
/// result**, matching Verilog's context-determined width rule.
///
/// # Why this exists
///
/// RTL `Binary` does not require its operands to be as wide as its
/// result. Shifts have always exercised that — a `Bits<8>` shift amount
/// against a 48-bit value — and since `XMul` stopped pre-widening its
/// operands, multiplies do too. What the operands *mean* in that case is
/// fixed by the emitted Verilog, where `*` is context-determined: both
/// operands are extended (per their own signedness) to the width of the
/// assignment target, and the operation is performed there.
///
/// So this resizes the operands to the result width before delegating,
/// which is precisely what the emitted hardware does. Without it the
/// interpreters would disagree with `iverilog` — [`super::vm`] would
/// compute a multiply at the *first* operand's width, because
/// `rhif::runtime_ops::mul` takes its result width from `a`.
///
/// # Why shifts and comparisons are excluded
///
/// Verilog does not context-extend either. A shift's right operand is a
/// count, not a value to align — widening it changes nothing and
/// narrowing the result would be wrong. A comparison's operands size to
/// each other rather than to its one-bit result, and `max(a, b, 1)` is
/// already `max(a, b)`, so the existing behaviour is correct and is left
/// alone.
///
/// # Why this is a no-op for every pre-existing program
///
/// Before this, the only RTL binaries with operands narrower than their
/// result were shifts, which are excluded. Every other binary was emitted
/// with all three widths equal, so `resize` to the result width returns
/// the operand unchanged. That is the property that makes this safe: it
/// cannot alter any program that does not use the new lowering, and
/// `cargo test --all` passing without re-blessing a single VCD digest is
/// the evidence.
pub fn binary_at_result_width(
    op: AluBinary,
    arg1: TypedBits,
    arg2: TypedBits,
    result_bits: usize,
) -> Result<TypedBits, RHDLError> {
    let widen = matches!(
        op,
        AluBinary::Add
            | AluBinary::Sub
            | AluBinary::Mul
            | AluBinary::BitXor
            | AluBinary::BitAnd
            | AluBinary::BitOr
    );
    if !widen {
        return binary(op, arg1, arg2);
    }
    let arg1 = if arg1.len() == result_bits {
        arg1
    } else {
        arg1.resize(result_bits)?
    };
    let arg2 = if arg2.len() == result_bits {
        arg2
    } else {
        arg2.resize(result_bits)?
    };
    binary(op, arg1, arg2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitx::BitX;
    use crate::types::kind::Kind;

    // `binary_at_result_width` is the one place the "operands may be
    // narrower than the result" rule is implemented, and both consumers --
    // `rtl::vm` and `rtl_passes::constant_propagation` -- delegate to it.
    //
    // The VM's use is covered end-to-end by the exhaustive `xmul` tests in
    // `crates/rhdl/tests/dyn_bits.rs`, which require the RHIF VM, the RTL
    // VM and `iverilog` to agree. The const-prop use is NOT reachable from
    // a kernel: RHIF constant propagation folds any two-literal `Binary`
    // before RTL lowering runs, so mutating that call site breaks no test.
    //
    // These tests exist so the shared logic is covered on its own terms
    // rather than only through the one consumer that can be reached.

    /// Little-endian bit vector of `value` at `bits` width.
    fn raw(value: u128, bits: usize) -> Vec<BitX> {
        (0..bits)
            .map(|i| {
                if (value >> i) & 1 == 1 {
                    BitX::One
                } else {
                    BitX::Zero
                }
            })
            .collect()
    }

    fn u(value: u128, bits: usize) -> TypedBits {
        TypedBits::new(raw(value, bits), Kind::make_bits(bits))
    }

    fn s(value: i128, bits: usize) -> TypedBits {
        let masked = (value as u128) & ((1u128 << bits) - 1);
        TypedBits::new(raw(masked, bits), Kind::make_signed(bits))
    }

    /// The narrow-operand multiply, unsigned: 4 x 3 bits into 7.
    ///
    /// Exhaustive, because it is cheap and because the interesting failure
    /// is a truncation that only shows at particular magnitudes.
    #[test]
    fn unsigned_mul_at_result_width_is_exact() {
        for a in 0u128..16 {
            for b in 0u128..8 {
                let got = binary_at_result_width(AluBinary::Mul, u(a, 4), u(b, 3), 7).unwrap();
                assert_eq!(got.len(), 7, "result width must be the declared one");
                assert_eq!(got.as_i64().unwrap() as u128, a * b, "{a} * {b} at width 7");
            }
        }
    }

    /// The narrow-operand multiply, signed: 4 x 3 bits into 7.
    ///
    /// Signed is the case that breaks first if the operands are read at the
    /// result width without sign extension — a narrow negative becomes a
    /// large positive. Sweeping both full ranges covers every sign
    /// combination including both maximum-negative values.
    #[test]
    fn signed_mul_at_result_width_is_exact() {
        for a in -8i128..8 {
            for b in -4i128..4 {
                let got = binary_at_result_width(AluBinary::Mul, s(a, 4), s(b, 3), 7).unwrap();
                assert_eq!(got.len(), 7, "result width must be the declared one");
                assert_eq!(got.as_i64().unwrap() as i128, a * b, "{a} * {b} at width 7");
            }
        }
    }

    /// Equal-width operands are untouched, which is why this change could
    /// not alter any pre-existing program.
    ///
    /// Every binary except a shift was emitted with all three widths equal
    /// before `XMul` stopped pre-widening, so the resize is a no-op and the
    /// result must match plain `binary` exactly.
    #[test]
    fn equal_width_operands_match_plain_binary() {
        for op in [
            AluBinary::Add,
            AluBinary::Sub,
            AluBinary::Mul,
            AluBinary::BitXor,
            AluBinary::BitAnd,
            AluBinary::BitOr,
        ] {
            for a in 0u128..16 {
                for b in 0u128..16 {
                    let want = binary(op, u(a, 8), u(b, 8)).unwrap();
                    let got = binary_at_result_width(op, u(a, 8), u(b, 8), 8).unwrap();
                    assert_eq!(got, want, "{op:?} on equal widths must be unchanged");
                }
            }
        }
    }

    /// Shifts are excluded: the right operand is a count, not a value to
    /// align, and Verilog does not context-extend it either.
    ///
    /// This is not a detail — RTL has always carried shifts with an 8-bit
    /// count against a wide value, so widening would change long-standing
    /// behaviour.
    ///
    /// **The result width is deliberately wider than the operands here.**
    /// An earlier version of this test used width 8 throughout, which made
    /// widening a no-op and the test vacuous — it passed even with the
    /// exclusion removed. Mutation testing caught that. With a 16-bit
    /// result, widening would preserve the shifted-out bits and change the
    /// answer, so the assertion has something to detect.
    #[test]
    fn shifts_are_not_widened() {
        for amount in 0u128..8 {
            let want = binary(AluBinary::Shl, u(0b1011_0011, 8), u(amount, 8)).unwrap();
            let got = binary_at_result_width(AluBinary::Shl, u(0b1011_0011, 8), u(amount, 8), 16)
                .unwrap();
            assert_eq!(
                got, want,
                "shl at amount {amount} must be evaluated at the operand \
                 width, not widened to the 16-bit result"
            );
            assert_eq!(got.len(), 8, "a shift keeps its left operand's width");
        }
    }

    /// Comparisons are excluded: their operands size to each other, not to
    /// the one-bit result, so widening to the result width would compare
    /// two single bits.
    #[test]
    fn comparisons_are_not_widened_to_their_one_bit_result() {
        for a in 0u128..8 {
            for b in 0u128..8 {
                for op in [AluBinary::Eq, AluBinary::Lt, AluBinary::Gt, AluBinary::Ne] {
                    let got = binary_at_result_width(op, u(a, 4), u(b, 4), 1).unwrap();
                    let want = binary(op, u(a, 4), u(b, 4)).unwrap();
                    assert_eq!(got, want, "{op:?}({a}, {b}) must not be widened to 1 bit");
                }
            }
        }
    }

    /// Add and subtract also honour a wider result, which is what makes
    /// this a general rule rather than a multiply special case.
    #[test]
    fn narrow_add_and_sub_at_result_width() {
        // 15 + 15 = 30 needs 5 bits; at width 4 it would wrap to 14.
        let got = binary_at_result_width(AluBinary::Add, u(15, 4), u(15, 4), 5).unwrap();
        assert_eq!(got.as_i64().unwrap(), 30);
        assert_eq!(got.len(), 5);
        // Signed: -8 + -8 = -16 needs 5 bits.
        let got = binary_at_result_width(AluBinary::Add, s(-8, 4), s(-8, 4), 5).unwrap();
        assert_eq!(got.as_i64().unwrap(), -16);
    }
}
