//! No RTL register has zero width.
//!
//! # The invariant
//!
//! A `Digital` type with no bits has exactly **one inhabitant**, so it
//! carries no information and is fully determined at compile time.
//! There is nothing for a register to hold. Materialising one is
//! therefore always either pointless or a symptom of a lowering that
//! forgot the zero-width case.
//!
//! # Why it is enforced rather than made impossible
//!
//! The obvious alternative is to have the lowering hand back a
//! zero-width *literal* instead of allocating a register — a
//! one-inhabitant type genuinely is a constant, so that is the honest
//! representation. But `operand()` is called for both reads and writes
//! and cannot tell them apart, so a write to a zero-width `lhs` would
//! silently target a literal. The twenty-one `lhs.is_empty()` guards in
//! the lowering should prevent that, and "should" is not a good enough
//! basis for a change that fails silently when it is wrong.
//!
//! Enforcing the invariant gives the same protection with a loud
//! failure mode: any construct that creates a zero-width register stops
//! the compile at the layer that created it.
//!
//! # Relationship to the other zero-width work
//!
//! This is the structural half. [`super::check_registers_are_written`]
//! is the behavioural half — it catches a register that is read without
//! being written, which is how the zero-width defect actually
//! manifested. Either one alone would have caught it; together they
//! close the class from both directions, and neither is specific to
//! zero width.

use crate::{
    RHDLError,
    compiler::mir::error::ICE,
    rtl::{Object, spec::Operand},
};

use super::pass::Pass;

#[derive(Default, Debug, Clone)]
pub struct CheckNoZeroWidthRegisters {}

impl Pass for CheckNoZeroWidthRegisters {
    fn run(input: Object) -> Result<Object, RHDLError> {
        if let Some((id, _)) = input
            .symtab
            .iter_reg()
            .find(|(_, (kind, _))| kind.is_empty())
        {
            return Err(Self::raise_ice(
                &input,
                ICE::RTLRegisterIsZeroWidth {
                    operand: Operand::Register(id),
                },
                input.symbols.fallback(input.fn_id),
            ));
        }
        Ok(input)
    }
    fn description() -> &'static str {
        "Check that no register has zero width"
    }
}
