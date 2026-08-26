use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use crate::{
    RHDLError,
    common::symtab::RegisterId,
    compiler::mir::error::ICE,
    error::rhdl_error,
    ntl::{
        Object,
        error::NetLoopError,
        spec::{OpCode, Wire, WireKind},
        visit::visit_wires,
    },
    types::path::{Path, bit_range, leaf_paths},
};

use super::pass::Pass;

#[derive(Default, Debug, Clone)]
pub struct ReorderInstructions {}

fn raise_cycle_error(
    input: &Object,
    elements: Vec<(Option<String>, miette::SourceSpan)>,
) -> RHDLError {
    rhdl_error(NetLoopError {
        src: input.code.source(),
        elements,
    })
}

impl Pass for ReorderInstructions {
    fn run(mut input: Object) -> Result<Object, RHDLError> {
        // An implementation of Kahn's algorithm
        // The set N contains the set of register values that are
        // required for the reordering to be successful
        let mut needed = BTreeSet::<RegisterId<WireKind>>::new();
        needed.extend(input.outputs.iter().copied().flat_map(Wire::reg));
        // The set S contains the working set of defined register values.
        let mut satisfied = VecDeque::<RegisterId<WireKind>>::default();
        // The vector P contains the ordering of op codes
        let mut scheduled = Vec::<usize>::default();
        // The set L contains the completed set of register values
        let mut finished = BTreeSet::<RegisterId<WireKind>>::default();
        // We start by pre-populating the satisfied set with all of the inputs
        satisfied.extend(input.inputs.iter().flatten());
        // Next we scan through all op-codes and pre-emit those that correspond
        // to black box invokations.  Since those are write-before read, we need
        // to treat them twice.
        input
            .ops
            .iter()
            .filter_map(|lop| match &lop.op {
                OpCode::BlackBox(blackbox) => Some(blackbox),
                _ => None,
            })
            .for_each(|black_box| {
                satisfied.extend(black_box.lhs.iter().copied().filter_map(Wire::reg));
                needed.extend(black_box.arg.iter().flatten().copied().flat_map(Wire::reg));
            });
        // Now, we create a pair of maps.  The first, maps each register to the set of
        // opcodes that depend on it.  The second maps each opcode to the set of registers
        // that it depends on.
        let mut reg_to_op = BTreeMap::<RegisterId<WireKind>, BTreeSet<usize>>::default();
        let mut op_to_read_regs = BTreeMap::<usize, BTreeSet<RegisterId<WireKind>>>::default();
        let mut write_regs_to_op = BTreeMap::<RegisterId<WireKind>, usize>::default();
        for (ndx, lop) in input.ops.iter().enumerate() {
            visit_wires(&lop.op, |sense, opnd| {
                if sense.is_read() {
                    if let Some(reg) = opnd.reg() {
                        reg_to_op.entry(reg).or_default().insert(ndx);
                        op_to_read_regs.entry(ndx).or_default().insert(reg);
                    }
                } else if let Some(reg) = opnd.reg() {
                    write_regs_to_op.insert(reg, ndx);
                }
            });
        }
        // Schedule any ops that do not depend on any inputs.  These are
        // op codes like comments, Noops, and op codes that take constants
        // as inputs (and probably should have been eliminated already).
        for (ndx, lop) in input.ops.iter().enumerate() {
            if !matches!(lop.op, OpCode::BlackBox(_)) && !op_to_read_regs.contains_key(&ndx) {
                scheduled.push(ndx);
                let op_code = &input.ops[ndx].op;
                visit_wires(op_code, |sense, operand| {
                    if sense.is_write()
                        && let Some(reg) = operand.reg()
                    {
                        satisfied.push_back(reg);
                    }
                });
            }
        }
        // Run the Kahn algorithm
        while let Some(n) = satisfied.pop_front() {
            finished.insert(n);
            let Some(dep_ops) = reg_to_op.remove(&n) else {
                // It is possible that no ops depend on a register
                continue;
            };
            for op in dep_ops {
                // The given opcode has a dependency on this register.
                // Remove the dependency
                let can_schedule = if let Some(deps) = op_to_read_regs.get_mut(&op) {
                    deps.remove(&n);
                    deps.is_empty()
                } else {
                    true
                };
                // If we can schedule this opcode, then add it to the scheduled list
                if can_schedule {
                    scheduled.push(op);
                    // Mark the outputs of this op code as satisfied, unless we are a black box
                    let op_code = &input.ops[op].op;
                    if !matches!(op_code, OpCode::BlackBox(_)) {
                        visit_wires(op_code, |sense, operand| {
                            if sense.is_write()
                                && let Some(reg) = operand.reg()
                            {
                                satisfied.push_back(reg);
                            }
                        });
                    }
                }
            }
        }
        // Hope springs eternal...
        if let Some(mut failed) = needed.iter().find(|r| !finished.contains(r)).copied() {
            // Isolate a loop
            let mut regs = VecDeque::new();
            let mut visited = HashSet::new();
            loop {
                regs.push_back(failed);
                visited.insert(failed);
                // This is the opcode that writes the missing reg.
                //
                // Checked, not indexed. `needed` holds the outputs and
                // the black-box arguments, so a needed register with no
                // writer means the netlist is *undriven* there rather
                // than cyclic -- and this loop-isolation code has no loop
                // to isolate. Indexing panicked, which is reachable:
                // `optimize_ntl` can eliminate the ops that drove a
                // needed register, and `CheckForUndriven` runs before
                // that rather than after.
                let Some(&opc) = write_regs_to_op.get(&failed) else {
                    return Err(Self::raise_ice(
                        &input,
                        ICE::NeededRegisterHasNoWriter,
                        None,
                    ));
                };
                // That opcode must be missing an argument (or it would have been scheduled already)
                let Some(&next) = op_to_read_regs[&opc].iter().next() else {
                    // This is an error, since if the op had no unsatisfied inputs
                    // it should have been scheduled.
                    return Err(Self::raise_ice(
                        &input,
                        ICE::LoopIsolationAlgorithmFailed,
                        None,
                    ));
                };
                if visited.contains(&next) {
                    // This reg is in the loop.  Discard regs from the
                    // list that come before this one
                    while !regs.is_empty() && regs.front() != Some(&next) {
                        regs.pop_front();
                    }
                    break;
                }
                failed = next;
            }
            if regs.is_empty() {
                return Err(Self::raise_ice(
                    &input,
                    ICE::LoopIsolationAlgorithmFailed,
                    None,
                ));
            }
            for reg in &regs {
                let op = write_regs_to_op[reg];
                let lop = &input.ops[op];
                log::error!("Failed opcode -> {:?}", lop.op);
                visit_wires(&lop.op, |_sense, wire| {
                    if let Some(lid) = wire.lit() {
                        log::error!("Literal {lid} -> {}", input.symtab[lid]);
                    }
                })
            }
            let mut diag = vec![];
            for reg in regs {
                let details = &input.symtab[Wire::Register(reg)];
                if let Some(source_details) = &details.source_details {
                    let kind = details.kind;
                    let paths = leaf_paths(&kind, Path::default());
                    if let Some(path) = paths.iter().find(|p| {
                        let Ok((bits1, _)) = bit_range(kind, p) else {
                            return false;
                        };
                        bits1.contains(&details.bit)
                    }) {
                        let value_description = if !path.is_empty() {
                            Some(format!("{path:?}"))
                        } else {
                            None
                        };
                        let span: miette::SourceSpan =
                            input.code.span(source_details.location).into();
                        diag.push((value_description, span));
                    }
                }
            }
            return Err(raise_cycle_error(&input, diag));
        }
        // Reorder and select
        let reordered = scheduled
            .into_iter()
            .map(|ndx| input.ops[ndx].clone())
            .collect();
        input.ops = reordered;
        Ok(input)
    }

    fn description() -> &'static str {
        "Reorder instructions to create legal dataflow"
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use crate::{
        ClockReset, Kind,
        compiler::mir::error::ICE,
        error::RHDLError,
        ntl::builder::{Builder, BuilderMode},
        types::digital::Digital,
    };

    /// Two output bits, each assigned from the other.
    ///
    /// A textbook cycle, and also *dead*: nothing outside the pair
    /// depends on either half, so `optimize_ntl` deletes both ops. Which
    /// of the two faults a given test sees therefore depends entirely on
    /// whether the optimiser has run.
    fn cross_assigned_outputs() -> Builder {
        let mut builder = Builder::new("cyclic");
        // A synchronous netlist's first input is the clock and reset.
        let _cr = builder.add_input(ClockReset::static_kind());
        let out = builder.allocate_outputs(Kind::Bits(2));
        builder.copy_from_to(out[0], out[1]);
        builder.copy_from_to(out[1], out[0]);
        builder
    }

    /// The netlist-level loop detector still fires.
    ///
    /// This exists because the pass had no test at all. Until the
    /// composition-level check in `circuit::reachability` landed,
    /// `crates/rhdl/tests/logic_loop.rs` was the only thing exercising it
    /// — and that check now reports a cycle first, from the widget's
    /// descriptor, so the end-to-end route is gone. The pass remains the
    /// backstop for any cycle the composition check cannot see, such as
    /// one through a combinational black box whose feedthrough is assumed
    /// rather than declared, and a backstop with no test is a backstop in
    /// name only.
    ///
    /// It runs on the *unoptimised* netlist because the optimiser would
    /// otherwise delete the cycle before this pass could find it.
    #[test]
    fn a_cyclic_netlist_is_reported_as_a_loop() {
        let obj = cross_assigned_outputs().into_unoptimized(BuilderMode::Synchronous);
        let result = ReorderInstructions::run(obj);
        assert!(
            matches!(result, Err(RHDLError::NetLoopError(_))),
            "expected a netlist loop error, got {:?}",
            result.map(|o| o.name)
        );
    }

    /// Through the full optimiser, the same netlist is reported as
    /// undriven — with a diagnostic, not an internal compiler error.
    ///
    /// This is the user-visible half of the fix. `optimize_ntl` deletes
    /// the two ops as dead, which leaves the outputs with no driver.
    /// `CheckForUndriven` used to check only registers that some op
    /// *read*, so an undriven *output* was invisible to it, and
    /// `ReorderInstructions` — whose `needed` set is built from the
    /// outputs — hit that state first and panicked. Now the checker knows
    /// about outputs and runs first.
    #[test]
    fn an_undriven_output_is_reported_as_undriven_not_as_an_ice() {
        let result = cross_assigned_outputs().build(BuilderMode::Synchronous);
        let Err(RHDLError::NetListError(err)) = result else {
            panic!(
                "expected an undriven-netlist error, got {:?}",
                result.map(|o| o.name)
            );
        };
        assert!(
            matches!(
                err.cause,
                crate::ntl::error::NetListICE::UndrivenNetlistNode
            ),
            "wrong cause: {:?}",
            err.cause
        );
    }

    /// And the guard behind it still holds.
    ///
    /// With the checker in front, `ReorderInstructions` should never meet
    /// a needed register that has no writer. It used to index
    /// `write_regs_to_op` directly and panic if it did. Reaching the guard
    /// now means driving the pass without the checker ahead of it, which
    /// is what this does — an ICE that nothing can reach is still worth
    /// keeping, and worth keeping tested, because the alternative if the
    /// ordering ever changes back is a crash.
    #[test]
    fn a_needed_register_with_no_writer_is_an_ice_not_a_panic() {
        let mut builder = Builder::new("undriven");
        let _cr = builder.add_input(ClockReset::static_kind());
        // An output with no driver at all, and no ops to delete.
        let _out = builder.allocate_outputs(Kind::Bits(1));
        let obj = builder.into_unoptimized(BuilderMode::Synchronous);
        let Err(RHDLError::RHDLInternalCompilerError(err)) = ReorderInstructions::run(obj) else {
            panic!("expected an internal compiler error");
        };
        assert!(
            matches!(err.cause, ICE::NeededRegisterHasNoWriter),
            "wrong cause: {:?}",
            err.cause
        );
    }
}
