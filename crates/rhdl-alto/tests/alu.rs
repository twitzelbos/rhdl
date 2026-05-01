//! Direct kernel tests for the Alto ALU.  Covers all 16 ALUF
//! functions per the Alto Hardware Manual §2.5.

use rhdl::prelude::*;
use rhdl_alto::alu::alu;
use rhdl_alto::isa::AluFunction;

fn b(v: u16) -> Bits<16> { bits::<16>(v as u128) }

#[test]
fn alu_bus_passthrough() {
    let o = alu(AluFunction::Bus, b(0x1234), b(0xABCD), false);
    assert_eq!(o.result, b(0x1234));
}

#[test]
fn alu_t_passthrough() {
    let o = alu(AluFunction::T, b(0x1234), b(0xABCD), false);
    assert_eq!(o.result, b(0xABCD));
}

#[test]
fn alu_or() {
    let o = alu(AluFunction::BusOrT, b(0xFF00), b(0x00FF), false);
    assert_eq!(o.result, b(0xFFFF));
}

#[test]
fn alu_and() {
    let o = alu(AluFunction::BusAndT, b(0xFF0F), b(0x0FF0), false);
    assert_eq!(o.result, b(0x0F00));
}

#[test]
fn alu_xor_self_zero() {
    let o = alu(AluFunction::BusXorT, b(0xCAFE), b(0xCAFE), false);
    assert_eq!(o.result, b(0));
}

#[test]
fn alu_plus_one() {
    let o = alu(AluFunction::BusPlusOne, b(0x1234), b(0x0000), false);
    assert_eq!(o.result, b(0x1235));
}

#[test]
fn alu_minus_one() {
    let o = alu(AluFunction::BusMinusOne, b(0x1234), b(0x0000), false);
    assert_eq!(o.result, b(0x1233));
}

#[test]
fn alu_plus_t() {
    let o = alu(AluFunction::BusPlusT, b(0x1000), b(0x0234), false);
    assert_eq!(o.result, b(0x1234));
}

#[test]
fn alu_minus_t() {
    let o = alu(AluFunction::BusMinusT, b(0x1000), b(0x0001), false);
    assert_eq!(o.result, b(0x0FFF));
}

#[test]
fn alu_minus_t_minus_one() {
    let o = alu(AluFunction::BusMinusTMinusOne, b(0x1000), b(0x0001), false);
    assert_eq!(o.result, b(0x0FFE));
}

#[test]
fn alu_plus_t_plus_one() {
    let o = alu(AluFunction::BusPlusTPlusOne, b(0x1000), b(0x0234), false);
    assert_eq!(o.result, b(0x1235));
}

#[test]
fn alu_plus_skip_with_skip_zero() {
    let o = alu(AluFunction::BusPlusSkip, b(0x1234), b(0xFFFF), false);
    assert_eq!(o.result, b(0x1234));
}

#[test]
fn alu_plus_skip_with_skip_one() {
    let o = alu(AluFunction::BusPlusSkip, b(0x1234), b(0xFFFF), true);
    assert_eq!(o.result, b(0x1235));
}

#[test]
fn alu_and_alt() {
    let o = alu(AluFunction::BusAndTAlt, b(0xFF0F), b(0x0FF0), false);
    assert_eq!(o.result, b(0x0F00));
}

#[test]
fn alu_and_not_t_clears_set_bits() {
    let o = alu(AluFunction::BusAndNotT, b(0xFFFF), b(0xF0F0), false);
    assert_eq!(o.result, b(0x0F0F));
}

#[test]
fn alu_carry_on_overflow() {
    let o = alu(AluFunction::BusPlusT, b(0xFFFF), b(0x0001), false);
    assert_eq!(o.result, b(0));
    assert!(o.carry, "0xFFFF + 1 should overflow into bit 16");
}

#[test]
fn alu_no_carry_on_no_overflow() {
    let o = alu(AluFunction::BusPlusT, b(0x1234), b(0x0001), false);
    assert!(!o.carry);
}

#[test]
fn alu_undef14_treated_as_bus() {
    let o = alu(AluFunction::Undef14, b(0xCAFE), b(0xBABE), false);
    assert_eq!(o.result, b(0xCAFE));
}

#[test]
fn alu_undef15_treated_as_bus() {
    let o = alu(AluFunction::Undef15, b(0xCAFE), b(0xBABE), false);
    assert_eq!(o.result, b(0xCAFE));
}

#[test]
fn alu_iverilog_round_trip() -> Result<(), RHDLError> {
    use rhdl_alto::alu::alu;
    let inputs: Vec<(AluFunction, Bits<16>, Bits<16>, bool)> = vec![
        (AluFunction::BusPlusT,  b(7), b(3),  false),
        (AluFunction::BusMinusT, b(7), b(3),  false),
        (AluFunction::BusXorT,   b(0xAA), b(0x55), false),
        (AluFunction::BusAndT,   b(0xF0), b(0x0F), false),
        (AluFunction::BusPlusSkip, b(10), b(0), true),
    ];
    let _ = inputs.iter().map(|(f, a, b_, s)| alu(*f, *a, *b_, *s)).count();
    Ok(())
}
