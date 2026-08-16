//! RV32I 32-bit arithmetic-logic unit.
//!
//! Pure combinational kernel: takes an [`AluOp`], two 32-bit
//! operands, and produces a 32-bit result.  Implements every ALU
//! operation needed for both R-type and I-type instructions, plus
//! the address-calculation `Add` used for loads/stores/JALR/AUIPC,
//! plus the `Pass` pseudo-op used for LUI.
//!
//! Shift operations use the low 5 bits of operand B as the shift
//! amount per the RV32I spec.

use crate::isa::AluOp;
use rhdl::prelude::*;

/// Compute the 32-bit ALU result for `op a b`.
///
/// SLT and SLTU produce 0 or 1 in bit 0; all other bits are 0.
#[kernel]
pub fn alu(op: AluOp, a: Bits<32>, b: Bits<32>) -> Bits<32> {
    // Shift amount: low 5 bits of b (RV32I rule).
    let shamt: Bits<32> = b & bits::<32>(0x1F);

    // Signed views for SLT and SRA.
    let a_signed: SignedBits<32> = a.as_signed();
    let b_signed: SignedBits<32> = b.as_signed();

    match op {
        AluOp::Add => a + b,
        AluOp::Sub => a - b,
        AluOp::Sll => a << shamt,
        AluOp::Slt => {
            if a_signed < b_signed {
                bits::<32>(1)
            } else {
                bits::<32>(0)
            }
        }
        AluOp::Sltu => {
            if a < b {
                bits::<32>(1)
            } else {
                bits::<32>(0)
            }
        }
        AluOp::Xor => a ^ b,
        AluOp::Srl => a >> shamt,
        AluOp::Sra => (a_signed >> shamt).as_unsigned(),
        AluOp::Or => a | b,
        AluOp::And => a & b,
        AluOp::Pass => b,
    }
}
