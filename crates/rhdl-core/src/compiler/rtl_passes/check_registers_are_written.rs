//! Every RTL register that is read must be written.
//!
//! # Why this exists
//!
//! `rhif_passes/` has `partial_initialization_check.rs` and
//! `check_rhif_flow.rs`. Nothing equivalent ran at RTL, so a lowering
//! that dropped a defining instruction produced an object whose reader
//! had nothing driving it — and the Verilog emitter faithfully rendered
//! that as an undriven `reg`, which reads as `x`.
//!
//! The Rust simulator does not go through Verilog, so it gave a defined
//! answer for the same design. **A silent divergence between the two
//! simulators**: it compiles, it passes every Rust-side test tier, and
//! only the Tier-4 `iverilog` round-trip notices.
//!
//! The concrete instance was zero-width types. `a.frame != b.frame` at
//! a zero-width `F` lowered to three instructions when `F` had bits and
//! to one when it did not — the two `Index` extractions were skipped,
//! correctly, since there are no bits to extract, but their destination
//! registers had already been allocated and were still referenced.
//!
//! # Why it is not a zero-width check
//!
//! Deliberately phrased as a general well-formedness invariant rather
//! than a special case. Zero width is how the hole was found; the hole
//! is that *any* dropped defining instruction was undetectable. This
//! turns that whole class into a compile error at the layer where it
//! happens, instead of an `x` in synthesis.

use std::collections::HashSet;

use crate::{
    RHDLError,
    common::sense::Sense,
    compiler::mir::error::ICE,
    rtl::{Object, spec::Operand, visit::visit_object_operands},
};

use super::pass::Pass;

#[derive(Default, Debug, Clone)]
pub struct CheckRegistersAreWritten {}

impl Pass for CheckRegistersAreWritten {
    fn run(input: Object) -> Result<Object, RHDLError> {
        let mut written: HashSet<Operand> = Default::default();
        let mut read: Vec<Operand> = Default::default();
        visit_object_operands(&input, |sense, operand| match sense {
            Sense::Write => {
                written.insert(*operand);
            }
            Sense::Read => read.push(*operand),
        });
        // Literals carry their value, so only registers can be
        // undriven.  Order the report by first appearance so the
        // message is stable across runs.
        let mut seen: HashSet<Operand> = Default::default();
        for operand in read {
            if !operand.is_reg() || written.contains(&operand) || !seen.insert(operand) {
                continue;
            }
            return Err(Self::raise_ice(
                &input,
                ICE::RTLRegisterReadButNeverWritten { operand },
                input.symbols.fallback(input.fn_id),
            ));
        }
        Ok(input)
    }
    fn description() -> &'static str {
        "Check that every register read is also written"
    }
}
