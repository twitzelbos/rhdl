//! Direct kernel tests for the Alto ALU.  Covers all 16 ALUF
//! functions per the Alto Hardware Manual §2.5.

use rhdl::prelude::*;
use rhdl_alto::alu::alu;
use rhdl_alto::isa::AluFunction;

fn b(v: u16) -> Bits<16> {
    bits::<16>(v as u128)
}

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

/// **Tier 4: the emitted Verilog agrees with the Rust model, for every
/// ALU function.**
///
/// This test previously bore this name and did no such thing: it called
/// `alu` five times in Rust, discarded the results with
/// `.map(..).count()`, and returned `Ok`. It therefore asserted
/// nothing at all -- not even that the outputs were right, let alone
/// that the hardware agreed -- so the Alto ALU had no Verilog
/// ground-truth check while appearing to have one. Clippy's
/// `suspicious_map` is what surfaced it.
///
/// It now runs the kernel through the RHDL VM *and* through iverilog
/// and requires all three to agree, across all sixteen functions and
/// both carry-in states.
#[test]
fn alu_iverilog_round_trip() -> Result<(), RHDLError> {
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;
    use rhdl_alto::alu::alu;

    const FUNCS: [AluFunction; 16] = [
        AluFunction::Bus,
        AluFunction::T,
        AluFunction::BusOrT,
        AluFunction::BusAndT,
        AluFunction::BusXorT,
        AluFunction::BusPlusOne,
        AluFunction::BusMinusOne,
        AluFunction::BusPlusT,
        AluFunction::BusMinusT,
        AluFunction::BusMinusTMinusOne,
        AluFunction::BusPlusTPlusOne,
        AluFunction::BusPlusSkip,
        AluFunction::BusAndTAlt,
        AluFunction::BusAndNotT,
        AluFunction::Undef14,
        AluFunction::Undef15,
    ];
    // Operand pairs chosen to exercise carry-out and borrow at their
    // boundaries, which is where an arithmetic function and its
    // Verilog are most likely to disagree.
    const OPERANDS: [(u16, u16); 8] = [
        (0, 0),
        (0, 1),
        (1, 0),
        (7, 3),
        (0xAAAA, 0x5555),
        (0xFFFF, 0x0001),
        (0x0001, 0xFFFF),
        (0xFFFF, 0xFFFF),
    ];

    let mut inputs = Vec::new();
    for f in FUNCS {
        for (x, y) in OPERANDS {
            for skip in [false, true] {
                inputs.push((f, b(x), b(y), skip));
            }
        }
    }
    assert_eq!(inputs.len(), 16 * 8 * 2);
    test_kernel_vm_and_verilog_synchronous::<alu, _, _, _>(alu, inputs.into_iter())?;
    Ok(())
}
