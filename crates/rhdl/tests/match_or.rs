//! End-to-end integration tests for or-patterns in `#[kernel]`
//! match arms.
//!
//! Or-patterns are desugared at the macro layer into one arm per
//! alternative (see `kernel.rs::match_ex`).  These tests verify
//! the round-trip semantics: a kernel using `A | B => body`
//! evaluates the same as a hand-written `A => body, B => body`,
//! and the lowered Verilog runs identically through both
//! `iverilog` and the in-tree VM.

#![allow(unused_variables)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use rhdl::prelude::*;
use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog;

#[derive(PartialEq, Default, Clone, Copy, Debug, Digital)]
pub enum Light {
    #[default]
    Red,
    Yellow,
    Green,
    Off,
}

/// Group `Red | Yellow` as "stop", `Green` as "go", `Off` as
/// "unknown".  Demonstrates the canonical or-pattern use case:
/// collapsing N variants with the same body into one arm.
#[kernel]
pub fn classify(state: Signal<Light, Red>) -> Signal<b2, Red> {
    let result = match state.val() {
        Light::Red | Light::Yellow => bits(1),
        Light::Green => bits(2),
        Light::Off => bits(0),
    };
    signal(result)
}

/// The hand-expanded equivalent of [`classify`].  Used as a
/// reference oracle in the equivalence test below.
#[kernel]
pub fn classify_expanded(state: Signal<Light, Red>) -> Signal<b2, Red> {
    let result = match state.val() {
        Light::Red => bits(1),
        Light::Yellow => bits(1),
        Light::Green => bits(2),
        Light::Off => bits(0),
    };
    signal(result)
}

#[test]
fn or_pattern_classify_matches_hand_expansion() -> miette::Result<()> {
    // Spot-check by direct kernel call — the simplest Tier-1
    // validation that the desugaring preserves semantics.
    for (variant, expected) in [
        (Light::Red, 1u128),
        (Light::Yellow, 1u128),
        (Light::Green, 2u128),
        (Light::Off, 0u128),
    ] {
        let or_result: u128 = classify(signal(variant)).val().raw();
        let hand_result: u128 = classify_expanded(signal(variant)).val().raw();
        assert_eq!(
            or_result, expected,
            "or-pattern classify({variant:?}) = {or_result}, expected {expected}",
        );
        assert_eq!(
            or_result, hand_result,
            "or-pattern classify({variant:?}) = {or_result}, hand-expanded = {hand_result}",
        );
    }
    Ok(())
}

#[test]
fn or_pattern_round_trips_through_verilog() -> miette::Result<()> {
    // Tier-4 validation: emit Verilog, run iverilog, verify cycle-for-cycle
    // agreement with the Rust simulator.  Sweeps all four Light variants.
    let inputs = [Light::Red, Light::Yellow, Light::Green, Light::Off];
    test_kernel_vm_and_verilog::<classify, _, _, _>(
        classify,
        inputs.into_iter().map(red).map(|x| (x,)),
    )?;
    Ok(())
}

#[derive(PartialEq, Default, Clone, Copy, Debug, Digital)]
pub enum Op {
    #[default]
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Three-way grouping: `Add | Sub` => arithmetic, `Mul | Div |
/// Mod` => multiplicative.  Verifies that more-than-two
/// alternatives flatten correctly (the macro produces an arm per
/// alternative, so this lowers to five arms — three under one
/// body, two under another).
#[kernel]
pub fn op_class(state: Signal<Op, Red>) -> Signal<b1, Red> {
    let result = match state.val() {
        Op::Add | Op::Sub => bits(0),
        Op::Mul | Op::Div | Op::Mod => bits(1),
    };
    signal(result)
}

#[test]
fn or_pattern_with_three_alternatives() -> miette::Result<()> {
    for (op, expected) in [
        (Op::Add, 0u128),
        (Op::Sub, 0u128),
        (Op::Mul, 1u128),
        (Op::Div, 1u128),
        (Op::Mod, 1u128),
    ] {
        let actual: u128 = op_class(signal(op)).val().raw();
        assert_eq!(actual, expected, "op_class({op:?}) = {actual}, expected {expected}");
    }
    Ok(())
}

#[test]
fn or_pattern_with_three_alternatives_through_verilog() -> miette::Result<()> {
    let inputs = [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Mod];
    test_kernel_vm_and_verilog::<op_class, _, _, _>(
        op_class,
        inputs.into_iter().map(red).map(|x| (x,)),
    )?;
    Ok(())
}

/// A subtler case: literal patterns inside an or-pattern.  Tests
/// that the desugaring doesn't accidentally treat the alternatives
/// as a single composite literal.
#[kernel]
pub fn opcode_class(opcode: Signal<b8, Red>) -> Signal<b1, Red> {
    let result = match opcode.val() {
        Bits::<8>(0x00) | Bits::<8>(0x01) | Bits::<8>(0x02) => bits(0),
        _ => bits(1),
    };
    signal(result)
}

#[test]
fn or_pattern_with_literal_alternatives() -> miette::Result<()> {
    for op in 0u128..=4u128 {
        let expected = if op <= 2 { 0 } else { 1 };
        let actual: u128 = opcode_class(signal(bits(op))).val().raw();
        assert_eq!(actual, expected, "opcode_class(0x{op:02x}) = {actual}, expected {expected}");
    }
    Ok(())
}

fn red<T>(x: T) -> Signal<T, Red>
where
    T: Digital,
{
    signal(x)
}
