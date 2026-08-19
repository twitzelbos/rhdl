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
