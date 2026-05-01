//! ISA compliance tests — runs hand-translated rv32ui-p-* tests
//! through both single-cycle and pipelined CPUs, asserts the
//! signature is 1 (pass).
//!
//! Each test corresponds to one upstream `riscv-tests` test
//! program, hand-translated.  See `crates/rhdl-rv32i/src/compliance.rs`
//! for the framework.
//!
//! Failure modes:
//! - Signature == 0: program didn't reach pass or fail handler
//!   (probably ran out of cycles).  Increase `max_cycles`.
//! - Signature == N (N > 1): sub-test N failed.  Check the
//!   sub-test's `(id=N, expected, a, b)` triple in the relevant
//!   `make_*_program` function.

use rhdl_rv32i::compliance::*;

/// Helper: assert both single-cycle and pipelined produce signature 1.
fn assert_compliance(program: Vec<u32>, max_cycles: usize, name: &str) {
    let single = run_signature_single(program.clone(), max_cycles);
    assert_eq!(
        single, 1,
        "{name}: single-cycle CPU failed sub-test (signature = {single})",
    );
    let pipelined = run_signature_pipelined(program, max_cycles * 2);
    assert_eq!(
        pipelined, 1,
        "{name}: pipelined CPU failed sub-test (signature = {pipelined})",
    );
}

#[test]
fn rv32ui_p_add() {
    // 15 sub-tests × ~7 instructions each + setup + handlers ≈ 200
    // cycles single-cycle, ~400 pipelined.
    assert_compliance(make_add_program(), 600, "rv32ui-p-add");
}

#[test]
fn rv32ui_p_sub() {
    assert_compliance(make_sub_program(), 600, "rv32ui-p-sub");
}

#[test]
fn rv32ui_p_and() {
    assert_compliance(make_and_program(), 600, "rv32ui-p-and");
}

#[test]
fn rv32ui_p_or() {
    assert_compliance(make_or_program(), 600, "rv32ui-p-or");
}

#[test]
fn rv32ui_p_xor() {
    assert_compliance(make_xor_program(), 600, "rv32ui-p-xor");
}

#[test]
fn rv32ui_p_addi() {
    assert_compliance(make_addi_program(), 800, "rv32ui-p-addi");
}
