use std::collections::HashSet;

use crate::{
    RHDLError,
    {
        common::symtab::RegisterId,
        error::rhdl_error,
        ntl::{
            Object,
            error::NetListError,
            spec::{Wire, WireKind},
            visit::visit_wires,
        },
    },
};

use super::pass::Pass;

#[derive(Default, Debug, Clone)]
pub struct CheckForUndriven {}

impl Pass for CheckForUndriven {
    fn description() -> &'static str {
        "Check For Undriven values"
    }
    fn run(input: Object) -> Result<Object, RHDLError> {
        let mut written_set: HashSet<RegisterId<WireKind>> = HashSet::default();
        for lop in &input.ops {
            visit_wires(&lop.op, |sense, op| {
                if sense.is_write()
                    && let Some(reg) = op.reg()
                {
                    written_set.insert(reg);
                }
            })
        }
        written_set.extend(input.inputs.iter().flatten().copied());
        // The outputs count as reads, and used not to be checked at all.
        //
        // Only registers that some op *reads* were verified, so an output
        // register that nothing writes and nothing reads was invisible
        // here. That is exactly the state `ReorderInstructions` cannot
        // cope with -- it builds its `needed` set from the outputs -- so
        // the one condition this pass missed was the one condition that
        // made the next pass fail, and it failed with an internal
        // compiler error rather than this diagnostic.
        //
        // Constant outputs are skipped: `Wire::reg` is `None` for a
        // literal, which needs no driver.
        for out in input.outputs.iter().copied().flat_map(Wire::reg) {
            if !written_set.contains(&out) {
                return Err(rhdl_error(NetListError {
                    cause: crate::ntl::error::NetListICE::UndrivenNetlistNode,
                    src: input.code.source(),
                    // No op to point at: the fault is the absence of one.
                    elements: Vec::new(),
                }));
            }
        }
        for lop in &input.ops {
            let mut err = None;
            visit_wires(&lop.op, |sense, op| {
                if sense.is_read()
                    && let Some(reg) = op.reg()
                    && !written_set.contains(&reg)
                {
                    log::warn!("{:?}", input);
                    err = Some(NetListError {
                        cause: crate::ntl::error::NetListICE::UndrivenNetlistNode,
                        src: input.code.source(),
                        elements: lop
                            .loc
                            .iter()
                            .map(|&loc| input.code.span(loc).into())
                            .collect(),
                    });
                }
            });
            if let Some(err) = err {
                return Err(rhdl_error(err));
            }
        }
        Ok(input)
    }
}
