//! ALU unit tests — one per [`AluOp`] variant.

use rhdl::prelude::*;
use rhdl_rv32i::alu::*;
use rhdl_rv32i::isa::AluOp;

fn b32(x: u32) -> Bits<32> {
    bits::<32>(x as u128)
}

#[test]
fn alu_add() {
    assert_eq!(alu(AluOp::Add, b32(3), b32(4)), b32(7));
    // Wraparound is unsigned 2's complement.
    assert_eq!(alu(AluOp::Add, b32(0xFFFF_FFFF), b32(1)), b32(0));
}

#[test]
fn alu_sub() {
    assert_eq!(alu(AluOp::Sub, b32(10), b32(4)), b32(6));
    assert_eq!(alu(AluOp::Sub, b32(0), b32(1)), b32(0xFFFF_FFFF));
}

#[test]
fn alu_sll_shift_amount_is_low_5_bits() {
    assert_eq!(alu(AluOp::Sll, b32(1), b32(4)), b32(16));
    // Bits above [4:0] of operand B are ignored per RV32I spec.
    assert_eq!(alu(AluOp::Sll, b32(1), b32(0x44)), b32(16));
    assert_eq!(alu(AluOp::Sll, b32(1), b32(31)), b32(0x8000_0000));
}

#[test]
fn alu_srl_unsigned_shift() {
    assert_eq!(alu(AluOp::Srl, b32(0x8000_0000), b32(31)), b32(1));
    assert_eq!(alu(AluOp::Srl, b32(0xFFFF_FFFF), b32(4)), b32(0x0FFF_FFFF));
}

#[test]
fn alu_sra_arithmetic_shift_preserves_sign() {
    // 0x8000_0000 = -2^31 in signed 32-bit.  Arithmetic shift right
    // by 1 = 0xC000_0000.
    assert_eq!(alu(AluOp::Sra, b32(0x8000_0000), b32(1)), b32(0xC000_0000));
    // Positive number: same as logical shift.
    assert_eq!(alu(AluOp::Sra, b32(0x4000_0000), b32(2)), b32(0x1000_0000));
    // Negative all-ones stays all-ones.
    assert_eq!(alu(AluOp::Sra, b32(0xFFFF_FFFF), b32(31)), b32(0xFFFF_FFFF));
}

#[test]
fn alu_slt_signed_compare() {
    // -1 < 0 → 1
    assert_eq!(alu(AluOp::Slt, b32(0xFFFF_FFFF), b32(0)), b32(1));
    // 0 < -1 → 0
    assert_eq!(alu(AluOp::Slt, b32(0), b32(0xFFFF_FFFF)), b32(0));
    // 5 < 10 → 1
    assert_eq!(alu(AluOp::Slt, b32(5), b32(10)), b32(1));
}

#[test]
fn alu_sltu_unsigned_compare() {
    // 0xFFFF_FFFF (unsigned big) > 0 → 0 (a < b is false)
    assert_eq!(alu(AluOp::Sltu, b32(0xFFFF_FFFF), b32(0)), b32(0));
    // 0 < 0xFFFF_FFFF → 1
    assert_eq!(alu(AluOp::Sltu, b32(0), b32(0xFFFF_FFFF)), b32(1));
}

#[test]
fn alu_xor_or_and() {
    assert_eq!(alu(AluOp::Xor, b32(0xF0F0), b32(0x0FF0)), b32(0xFF00));
    assert_eq!(alu(AluOp::Or, b32(0xF0F0), b32(0x0FF0)), b32(0xFFF0));
    assert_eq!(alu(AluOp::And, b32(0xF0F0), b32(0x0FF0)), b32(0x00F0));
}

#[test]
fn alu_pass_returns_b() {
    // Pass is the LUI helper: rd = 0 + imm = imm.
    assert_eq!(alu(AluOp::Pass, b32(0xDEADBEEF), b32(0x1234_0000)), b32(0x1234_0000));
}
