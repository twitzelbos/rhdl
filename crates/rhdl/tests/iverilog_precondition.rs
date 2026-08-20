//! Icarus Verilog precondition for this crate's integration tests.
//!
//! Each file under `tests/` is its own test binary, so a precondition in
//! the library's unit tests cannot abort them. This file is the check for
//! the integration suite.
//!
//! # Scope, stated honestly
//!
//! One file per binary would be needed for *complete* coverage, and there
//! are around thirty here. This is one binary's worth. Combined with the
//! library-level checks in `rhdl-core`, `rhdl-fpga`, `rhdl-alto`,
//! `rhdl-rule`, `rhdl-rv32i`, `rhdl-vlog` and the book's `code` crate — and
//! with `cargo test` being fail-fast across binaries — a run on a machine
//! without the toolchain stops early with one actionable message rather
//! than several hundred `NotFound` panics.
//!
//! It is not a guarantee that *no* raw panic can appear: if cargo happens
//! to reach another integration binary first, that binary's Tier-4 tests
//! panic in the old way before any precondition runs. Making that
//! impossible means a check per binary, which is a mechanical change worth
//! doing separately if the ordering ever bites.

#[test]
fn a_working_iverilog_is_present() {
    rhdl::vlog::toolchain::require_iverilog();
}
