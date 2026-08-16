//! Programmatic well-formedness checkers for RHIF [`Object`]s.
//!
//! This module is the executable counterpart to the prose specification at
//! [`doc/rhif-spec/invariants/object.md`](../../../../doc/rhif-spec/invariants/object.md).
//! Each function below corresponds to one normative invariant.  Together
//! they form the Phase 2 (per `rhif-formalization-plan.md`) property
//! oracle for a single `Object`.
//!
//! Every checker returns a `Vec<Violation>`: empty means "this invariant
//! holds."  The umbrella [`check_object`] runs every checker and bundles
//! the violations into a [`WellFormednessReport`].
//!
//! All checkers are pure: they read the `Object` and never mutate it.
//! They are designed to be cheap enough to run on every widget in the
//! corpus on every PR.

use std::collections::HashSet;
use std::fmt;

use crate::Kind;
use crate::common::sense::Sense;
use crate::common::symtab::{LiteralId, RegisterId};
use crate::rhif::object::Object;
use crate::rhif::spec::{CaseArgument, Cast, FuncId, OpCode, Slot, SlotKind, Wrap};
use crate::rhif::visit::{visit_object_slots, visit_slots};
use crate::types::path::PathElement;

/// A specific violation of one of the object-level invariants documented
/// in `doc/rhif-spec/invariants/object.md`.  Each variant carries enough
/// context to identify the offending opcode / slot / kind for diagnostic
/// reporting.
#[derive(Debug, Clone, PartialEq)]
pub enum Violation {
    /// A register slot is the `lhs` of more than one opcode.  Violates
    /// the single-assignment invariant.
    DoubleAssignment {
        register: RegisterId<SlotKind>,
        first_opcode_index: usize,
        second_opcode_index: usize,
    },
    /// An opcode reads a register that has not yet been defined.
    /// Violates def-before-use.
    UseBeforeDefinition {
        register: RegisterId<SlotKind>,
        opcode_index: usize,
    },
    /// An opcode references a register slot that is not in the symbol
    /// table.  Violates symbol-table completeness.
    UnregisteredRegister { register: RegisterId<SlotKind> },
    /// An opcode references a literal slot that is not in the symbol
    /// table.  Violates symbol-table completeness.
    UnregisteredLiteral { literal: LiteralId<SlotKind> },
    /// An opcode has `lhs = Slot::Literal(_)`.  Literals are read-only
    /// per the literal-read-only invariant.
    WriteToLiteral {
        literal: LiteralId<SlotKind>,
        opcode_index: usize,
    },
    /// A register or literal slot has a kind `Signal(Signal(_, _), _)` —
    /// i.e., a nested signal.  Violates the no-nested-signal invariant.
    NestedSignal { slot: Slot, kind: Kind },
    /// An `Exec` opcode references a `FuncId` that has no entry in
    /// `Object::externals`.  Violates externals-consistency.
    UnknownExternal {
        func_id: FuncId,
        opcode_index: usize,
    },
    /// An `Exec` opcode passes the wrong number of arguments to its
    /// callee.  Violates externals-consistency.
    ArgumentCountMismatch {
        func_id: FuncId,
        expected: usize,
        actual: usize,
        opcode_index: usize,
    },
    /// An `AsBits`, `AsSigned`, or `Resize` cast has `len = None`.  Violates
    /// the lowering-time invariant established by `lower_inferred_casts`.
    /// (Permitted in early IR; an ICE at the VM.)
    UnresolvedCastLength { opcode_index: usize },
    /// A `Retime` op has `color = None`.  Violates the lowering-time
    /// invariant established by `lower_inferred_retimes`.
    UnresolvedRetimeColor { opcode_index: usize },
    /// A `Wrap` op has `kind = None`.  An ICE at the VM if it survives
    /// inference.
    UnresolvedWrapKind { opcode_index: usize },
    /// `Object::return_slot` is a register that is neither in the symbol
    /// table nor produced by an opcode.  Violates the return-slot kind
    /// invariant.
    InvalidReturnSlot { slot: Slot },
    /// An argument register is not in the symbol table.
    InvalidArgumentRegister { register: RegisterId<SlotKind> },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::DoubleAssignment {
                register,
                first_opcode_index,
                second_opcode_index,
            } => write!(
                f,
                "register {register:?} is written by both opcode #{first_opcode_index} and \
                 opcode #{second_opcode_index} (single-assignment violation)",
            ),
            Violation::UseBeforeDefinition {
                register,
                opcode_index,
            } => write!(
                f,
                "register {register:?} is read by opcode #{opcode_index} before it is defined \
                 (def-before-use violation)",
            ),
            Violation::UnregisteredRegister { register } => write!(
                f,
                "register {register:?} is referenced by an opcode but is not in the symbol \
                 table (symbol-table-completeness violation)",
            ),
            Violation::UnregisteredLiteral { literal } => write!(
                f,
                "literal {literal:?} is referenced by an opcode but is not in the symbol \
                 table (symbol-table-completeness violation)",
            ),
            Violation::WriteToLiteral {
                literal,
                opcode_index,
            } => write!(
                f,
                "literal {literal:?} is the lhs of opcode #{opcode_index} (literal-read-only \
                 violation)",
            ),
            Violation::NestedSignal { slot, kind } => write!(
                f,
                "slot {slot:?} has a nested-signal kind {kind:?} (no-nested-signal violation)",
            ),
            Violation::UnknownExternal {
                func_id,
                opcode_index,
            } => write!(
                f,
                "opcode #{opcode_index} references unknown func id {func_id:?} \
                 (externals-consistency violation)",
            ),
            Violation::ArgumentCountMismatch {
                func_id,
                expected,
                actual,
                opcode_index,
            } => write!(
                f,
                "opcode #{opcode_index} calls func {func_id:?} with {actual} args, expected \
                 {expected} (externals-consistency violation)",
            ),
            Violation::UnresolvedCastLength { opcode_index } => write!(
                f,
                "opcode #{opcode_index} is a cast with len = None at lowering time \
                 (unresolved-cast-length violation; should have been resolved by \
                 lower_inferred_casts)",
            ),
            Violation::UnresolvedRetimeColor { opcode_index } => write!(
                f,
                "opcode #{opcode_index} is a Retime with color = None at lowering time \
                 (unresolved-retime-color violation; should have been resolved by \
                 lower_inferred_retimes)",
            ),
            Violation::UnresolvedWrapKind { opcode_index } => write!(
                f,
                "opcode #{opcode_index} is a Wrap with kind = None at lowering time \
                 (unresolved-wrap-kind violation)",
            ),
            Violation::InvalidReturnSlot { slot } => {
                write!(f, "Object::return_slot {slot:?} is not in the symbol table",)
            }
            Violation::InvalidArgumentRegister { register } => write!(
                f,
                "argument register {register:?} is not in the symbol table",
            ),
        }
    }
}

/// Aggregate result of running every well-formedness checker against
/// an `Object`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WellFormednessReport {
    pub violations: Vec<Violation>,
}

impl WellFormednessReport {
    /// Was the object fully well-formed?
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        self.violations.is_empty()
    }

    /// Filter to violations of a specific class — useful when a test
    /// only cares about one invariant.
    pub fn filter<F: Fn(&Violation) -> bool>(&self, f: F) -> Vec<&Violation> {
        self.violations.iter().filter(|v| f(v)).collect()
    }

    /// Number of distinct violations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.violations.len()
    }

    /// True iff there are no violations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }
}

impl fmt::Display for WellFormednessReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.violations.is_empty() {
            return write!(f, "<well-formed>");
        }
        writeln!(f, "{} well-formedness violation(s):", self.violations.len())?;
        for v in &self.violations {
            writeln!(f, "  - {v}")?;
        }
        Ok(())
    }
}

/// Run every well-formedness checker on `obj` and return the bundled
/// report.  This is the canonical entry point — equivalent to running
/// each per-invariant checker individually and concatenating the
/// results.
///
/// **Note.** Some invariants are documented in the spec as late-stage
/// only — specifically the "unresolved holes" check, which can only
/// be expected to hold after `lower_inferred_casts` /
/// `lower_inferred_retimes` have run.  This umbrella `check_object`
/// runs every checker; use [`check_object_universal`] when you need
/// invariants that hold at *every* pipeline checkpoint (including the
/// initial post-`infer` Object).  See `doc/rhif-spec/invariants/passes.md`
/// for the per-pass `Establishes` matrix.
#[must_use]
pub fn check_object(obj: &Object) -> WellFormednessReport {
    let mut violations = Vec::new();
    violations.extend(check_single_assignment(obj));
    violations.extend(check_def_before_use(obj));
    violations.extend(check_symbol_table_completeness(obj));
    violations.extend(check_no_literal_writes(obj));
    violations.extend(check_no_nested_signal(obj));
    violations.extend(check_externals_consistency(obj));
    violations.extend(check_unresolved_holes(obj));
    violations.extend(check_arguments_and_return(obj));
    WellFormednessReport { violations }
}

/// Run only the invariants that hold at *every* checkpoint of the
/// stage1 pipeline — including the initial post-`infer` Object,
/// before any lowering pass runs.  Excludes the "unresolved holes"
/// check, which is a late-stage invariant.
///
/// Use this when verifying per-pass well-formedness in a property
/// test.  Use [`check_object`] when verifying the final pipeline
/// output.
#[must_use]
pub fn check_object_universal(obj: &Object) -> WellFormednessReport {
    let mut violations = Vec::new();
    violations.extend(check_single_assignment(obj));
    violations.extend(check_def_before_use(obj));
    violations.extend(check_symbol_table_completeness(obj));
    violations.extend(check_no_literal_writes(obj));
    violations.extend(check_no_nested_signal(obj));
    violations.extend(check_externals_consistency(obj));
    violations.extend(check_arguments_and_return(obj));
    WellFormednessReport { violations }
}

/// Check the **single-assignment** invariant.  A register slot may be
/// written by at most one opcode; arguments are bound by the caller and
/// don't count.
#[must_use]
pub fn check_single_assignment(obj: &Object) -> Vec<Violation> {
    let mut first_writer: std::collections::HashMap<RegisterId<SlotKind>, usize> =
        Default::default();
    let mut violations = Vec::new();
    for (i, lop) in obj.ops.iter().enumerate() {
        if let Some(Slot::Register(r)) = lop.op.lhs() {
            if let Some(&first) = first_writer.get(&r) {
                violations.push(Violation::DoubleAssignment {
                    register: r,
                    first_opcode_index: first,
                    second_opcode_index: i,
                });
            } else {
                first_writer.insert(r, i);
            }
        }
    }
    violations
}

/// Check the **definition-before-use** invariant.  Every read of a
/// register precedes the opcode that defines it.  Argument slots are
/// considered defined at index 0.
#[must_use]
pub fn check_def_before_use(obj: &Object) -> Vec<Violation> {
    let mut defined: HashSet<RegisterId<SlotKind>> = obj.arguments.iter().copied().collect();
    let mut violations = Vec::new();
    for (i, lop) in obj.ops.iter().enumerate() {
        // Reads first (a Splice/Index reading from a slot that is also
        // defined later: if the read is in this same op, it must have
        // been defined before this op).  Per the visitor, reads and
        // writes in one op are reported as separate Sense::Read /
        // Sense::Write callbacks; we order them as "all reads first,
        // then the lhs write."
        let mut reads: Vec<RegisterId<SlotKind>> = Vec::new();
        let mut writes: Vec<RegisterId<SlotKind>> = Vec::new();
        visit_slots(&lop.op, |sense, slot| {
            if let Slot::Register(r) = slot {
                match sense {
                    Sense::Read => reads.push(*r),
                    Sense::Write => writes.push(*r),
                }
            }
        });
        for r in reads {
            if !defined.contains(&r) {
                violations.push(Violation::UseBeforeDefinition {
                    register: r,
                    opcode_index: i,
                });
            }
        }
        for r in writes {
            defined.insert(r);
        }
    }
    violations
}

/// Check **symbol-table completeness**: every slot referenced by any
/// opcode (or by `Object::return_slot` or `Object::arguments`) is
/// registered in `Object::symtab`.
#[must_use]
pub fn check_symbol_table_completeness(obj: &Object) -> Vec<Violation> {
    let valid_regs: HashSet<RegisterId<SlotKind>> = obj.symtab.iter_reg().map(|(r, _)| r).collect();
    let valid_lits: HashSet<LiteralId<SlotKind>> = obj.symtab.iter_lit().map(|(l, _)| l).collect();

    let mut violations = Vec::new();
    let mut seen_unregistered: HashSet<Slot> = HashSet::new();
    let mut report_slot = |slot: &Slot, violations: &mut Vec<Violation>| {
        if seen_unregistered.contains(slot) {
            return;
        }
        match slot {
            Slot::Register(r) => {
                if !valid_regs.contains(r) {
                    violations.push(Violation::UnregisteredRegister { register: *r });
                    seen_unregistered.insert(*slot);
                }
            }
            Slot::Literal(l) => {
                if !valid_lits.contains(l) {
                    violations.push(Violation::UnregisteredLiteral { literal: *l });
                    seen_unregistered.insert(*slot);
                }
            }
        }
    };

    visit_object_slots(obj, |_, slot| report_slot(slot, &mut violations));

    // Return slot.
    report_slot(&obj.return_slot, &mut violations);

    // Argument registers.
    for r in &obj.arguments {
        let slot = Slot::Register(*r);
        report_slot(&slot, &mut violations);
    }

    violations
}

/// Check **literal-read-only**: no opcode in `Object::ops` has
/// `lhs = Slot::Literal(_)`.
#[must_use]
pub fn check_no_literal_writes(obj: &Object) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (i, lop) in obj.ops.iter().enumerate() {
        if let Some(Slot::Literal(l)) = lop.op.lhs() {
            violations.push(Violation::WriteToLiteral {
                literal: l,
                opcode_index: i,
            });
        }
    }
    violations
}

/// Check **no-nested-signal**: for every slot in the symbol table,
/// if its kind is `Signal(T', _)`, then `T'` is not itself
/// `Signal(_, _)`.
#[must_use]
pub fn check_no_nested_signal(obj: &Object) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (rid, (kind, _)) in obj.symtab.iter_reg() {
        if let Some(inner_kind) = kind.signal_kind() {
            if inner_kind.is_signal() {
                violations.push(Violation::NestedSignal {
                    slot: Slot::Register(rid),
                    kind: *kind,
                });
            }
        }
    }
    for (lid, (tb, _)) in obj.symtab.iter_lit() {
        let kind = tb.kind();
        if let Some(inner_kind) = kind.signal_kind() {
            if inner_kind.is_signal() {
                violations.push(Violation::NestedSignal {
                    slot: Slot::Literal(lid),
                    kind,
                });
            }
        }
    }
    violations
}

/// Check **externals-consistency**: every `Exec(_, id, args)` opcode
/// references a `FuncId` present in `Object::externals`, and the arg
/// count matches the callee's `Object::arguments` length.
///
/// (Recursive callee well-formedness is _not_ checked here — the
/// recursion is left to the caller / framework.  Acyclicity of the call
/// graph is also not checked here; the front-end forbids recursion.)
#[must_use]
pub fn check_externals_consistency(obj: &Object) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (i, lop) in obj.ops.iter().enumerate() {
        if let OpCode::Exec(exec) = &lop.op {
            match obj.externals.get(&exec.id) {
                Some(callee) => {
                    if exec.args.len() != callee.arguments.len() {
                        violations.push(Violation::ArgumentCountMismatch {
                            func_id: exec.id,
                            expected: callee.arguments.len(),
                            actual: exec.args.len(),
                            opcode_index: i,
                        });
                    }
                }
                None => {
                    violations.push(Violation::UnknownExternal {
                        func_id: exec.id,
                        opcode_index: i,
                    });
                }
            }
        }
    }
    violations
}

/// Check that no opcode at lowering time still has an unresolved hole:
/// a `Cast` with `len = None`, a `Retime` with `color = None`, or a
/// `Wrap` with `kind = None`.  Such holes are permitted in early IR but
/// must be resolved before the VM runs.
#[must_use]
pub fn check_unresolved_holes(obj: &Object) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (i, lop) in obj.ops.iter().enumerate() {
        match &lop.op {
            OpCode::AsBits(Cast { len: None, .. })
            | OpCode::AsSigned(Cast { len: None, .. })
            | OpCode::Resize(Cast { len: None, .. }) => {
                violations.push(Violation::UnresolvedCastLength { opcode_index: i });
            }
            OpCode::Retime(retime) if retime.color.is_none() => {
                violations.push(Violation::UnresolvedRetimeColor { opcode_index: i });
            }
            OpCode::Wrap(Wrap { kind: None, .. }) => {
                violations.push(Violation::UnresolvedWrapKind { opcode_index: i });
            }
            _ => {}
        }
    }
    violations
}

/// Check that `Object::arguments` and `Object::return_slot` reference
/// valid entries of `Object::symtab`.
#[must_use]
pub fn check_arguments_and_return(obj: &Object) -> Vec<Violation> {
    let valid_regs: HashSet<RegisterId<SlotKind>> = obj.symtab.iter_reg().map(|(r, _)| r).collect();
    let valid_lits: HashSet<LiteralId<SlotKind>> = obj.symtab.iter_lit().map(|(l, _)| l).collect();

    let mut violations = Vec::new();
    for r in &obj.arguments {
        if !valid_regs.contains(r) {
            violations.push(Violation::InvalidArgumentRegister { register: *r });
        }
    }
    match obj.return_slot {
        Slot::Register(r) => {
            if !valid_regs.contains(&r) {
                violations.push(Violation::InvalidReturnSlot {
                    slot: obj.return_slot,
                });
            }
        }
        Slot::Literal(l) => {
            if !valid_lits.contains(&l) {
                violations.push(Violation::InvalidReturnSlot {
                    slot: obj.return_slot,
                });
            }
        }
    }
    violations
}

/// Compile a synchronous widget's kernel through `compile_design_stage1`
/// and verify that *every intermediate* RHIF `Object` (after every
/// pass) is well-formed under every invariant in this module.  The
/// per-pass discipline matches `rhif-formalization-plan.md` §5.1
/// "well-typedness preservation" — every pass takes a well-typed
/// Object and produces a well-typed Object.
///
/// Returns `Ok(())` if every checkpoint produced a well-formed
/// Object.  Returns `Err(...)` naming the first pass at which a
/// violation appeared.  Compile errors propagate as their own
/// `RHDLError`.
pub fn check_widget_well_formed_synchronous<W>() -> Result<(), String>
where
    W: crate::circuit::synchronous::SynchronousIO,
{
    let report = super::property_tests::run_per_pass_well_formedness::<W::Kernel>(
        crate::CompilationMode::Synchronous,
    )
    .map_err(|e| format!("compile failed: {e:?}"))?;
    if report.all_well_formed() {
        Ok(())
    } else {
        let (pass, r) = report.first_violation().unwrap();
        Err(format!(
            "first violation at pass `{pass}` (checkpoint #{}/{}):\n{r}",
            report
                .checkpoints
                .iter()
                .position(|(p, _)| p == pass)
                .unwrap_or(0),
            report.checkpoints.len(),
        ))
    }
}

/// As [`check_widget_well_formed_synchronous`] but for asynchronous
/// (multi-domain) widgets.
pub fn check_widget_well_formed_asynchronous<W>() -> Result<(), String>
where
    W: crate::circuit::circuit_impl::CircuitIO,
{
    let report = super::property_tests::run_per_pass_well_formedness::<W::Kernel>(
        crate::CompilationMode::Asynchronous,
    )
    .map_err(|e| format!("compile failed: {e:?}"))?;
    if report.all_well_formed() {
        Ok(())
    } else {
        let (pass, r) = report.first_violation().unwrap();
        Err(format!(
            "first violation at pass `{pass}` (checkpoint #{}/{}):\n{r}",
            report
                .checkpoints
                .iter()
                .position(|(p, _)| p == pass)
                .unwrap_or(0),
            report.checkpoints.len(),
        ))
    }
}

/// True iff every static `Path` referenced by an `Index` or `Splice`
/// opcode could in principle walk a sub-kind of the operand's kind.
/// This is a structural check that does not require a full type
/// inference; we only verify that path elements are "shape-compatible"
/// with the slot kind chain.
///
/// **Note.** This is a partial check — verifying full path-walk
/// well-formedness requires the type-system rules in
/// `doc/rhif-spec/type-system.md` and is the responsibility of the
/// `check_rhif_type` pass at compile time.  This helper only checks
/// the structural-shape constraints (e.g., `EnumPayload` must follow
/// an `Enum` kind, not a `Bits` kind) when those constraints are
/// statically obvious from the kind alone.
///
/// At present this is a placeholder for a future Phase-2 enhancement;
/// it returns no violations.  Wire-up to a real path-shape walker is
/// tracked as a Phase 2 follow-up.
#[must_use]
pub fn check_path_well_formedness(_obj: &Object) -> Vec<Violation> {
    let _ = PathElement::SignalValue; // suppress unused-import warning
    let _ = CaseArgument::Wild; // suppress unused-import warning
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypedBits;
    use crate::ast::SourceLocation;
    use crate::ast::ast_impl::FunctionId;
    use crate::common::symtab::SymbolTable;
    use crate::rhif::object::{LocatedOpCode, SourceDetails, SymbolMap};
    use crate::rhif::spec::{AluBinary, AluUnary, Assign, Binary, Unary};

    fn fid() -> FunctionId {
        FunctionId::from(0u64)
    }

    fn empty_loc() -> SourceLocation {
        SourceLocation {
            node: crate::ast::ast_impl::NodeId::new(0),
            func: fid(),
        }
    }

    fn empty_meta() -> SourceDetails {
        SourceDetails {
            location: empty_loc(),
            name: None,
        }
    }

    /// A minimally well-formed Object: one argument register, identity
    /// kernel that returns it.
    fn identity_object() -> Object {
        let mut symtab: SymbolTable<TypedBits, Kind, SourceDetails, SlotKind> =
            SymbolTable::default();
        let arg = match symtab.reg(Kind::Bits(8), empty_meta()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        Object {
            symbols: SymbolMap::default(),
            symtab,
            return_slot: Slot::Register(arg),
            externals: Default::default(),
            ops: Vec::new(),
            arguments: vec![arg],
            name: "identity".to_string(),
            fn_id: fid(),
            flags: Vec::new(),
        }
    }

    /// A degenerate Object whose return-slot is a register that is not
    /// in the symbol table.  Used to verify the InvalidReturnSlot check.
    fn degenerate_return_object() -> Object {
        // Build a symtab from a different SymbolTable so the IDs
        // are foreign keys.
        let mut bogus: SymbolTable<TypedBits, Kind, SourceDetails, SlotKind> =
            SymbolTable::default();
        let bogus_reg = match bogus.reg(Kind::Bits(8), empty_meta()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        Object {
            symbols: SymbolMap::default(),
            symtab: SymbolTable::default(),
            return_slot: Slot::Register(bogus_reg),
            externals: Default::default(),
            ops: Vec::new(),
            arguments: Vec::new(),
            name: "degenerate".to_string(),
            fn_id: fid(),
            flags: Vec::new(),
        }
    }

    #[test]
    fn degenerate_return_object_detected() {
        let obj = degenerate_return_object();
        let report = check_object(&obj);
        assert!(
            !report.is_well_formed(),
            "expected violations on degenerate return slot",
        );
        assert!(
            report
                .violations
                .iter()
                .any(|v| matches!(v, Violation::InvalidReturnSlot { .. })),
            "expected InvalidReturnSlot, got {report}",
        );
    }

    #[test]
    fn identity_object_is_well_formed() {
        let obj = identity_object();
        let report = check_object(&obj);
        assert!(
            report.is_well_formed(),
            "expected well-formed but got: {report}"
        );
    }

    #[test]
    fn detects_double_assignment() {
        let mut obj = identity_object();
        // Add a register, write it twice with two `Unary(Not, ...)`
        // opcodes that take the argument as input.
        let arg = obj.arguments[0];
        let r2 = match obj.symtab.reg(Kind::Bits(8), empty_meta()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        let lop1 = LocatedOpCode {
            op: OpCode::Unary(Unary {
                op: AluUnary::Not,
                lhs: Slot::Register(r2),
                arg1: Slot::Register(arg),
            }),
            loc: empty_loc(),
        };
        let lop2 = LocatedOpCode {
            op: OpCode::Unary(Unary {
                op: AluUnary::Not,
                lhs: Slot::Register(r2),
                arg1: Slot::Register(arg),
            }),
            loc: empty_loc(),
        };
        obj.ops.push(lop1);
        obj.ops.push(lop2);
        let v = check_single_assignment(&obj);
        assert_eq!(v.len(), 1);
        assert!(matches!(v[0], Violation::DoubleAssignment { .. }));
    }

    #[test]
    fn detects_use_before_definition() {
        let mut obj = identity_object();
        // Add a register, write it AFTER reading it.
        let arg = obj.arguments[0];
        let r2 = match obj.symtab.reg(Kind::Bits(8), empty_meta()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        let r3 = match obj.symtab.reg(Kind::Bits(8), empty_meta()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        // Read r2 (not yet defined) into r3.
        obj.ops.push(LocatedOpCode {
            op: OpCode::Assign(Assign {
                lhs: Slot::Register(r3),
                rhs: Slot::Register(r2),
            }),
            loc: empty_loc(),
        });
        // Define r2 (too late).
        obj.ops.push(LocatedOpCode {
            op: OpCode::Assign(Assign {
                lhs: Slot::Register(r2),
                rhs: Slot::Register(arg),
            }),
            loc: empty_loc(),
        });
        let v = check_def_before_use(&obj);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(matches!(v[0], Violation::UseBeforeDefinition { .. }));
    }

    #[test]
    fn detects_unregistered_register() {
        let mut obj = identity_object();
        // Push an opcode referencing a register that is not in the
        // symbol table.  We synthesise a bogus RegisterId by hand.
        let arg = obj.arguments[0];
        let bogus_reg = match SymbolTable::<TypedBits, Kind, SourceDetails, SlotKind>::default()
            .reg(Kind::Bits(8), empty_meta())
        {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        // bogus_reg is a key into a different SymbolTable; not in `obj.symtab`.
        obj.ops.push(LocatedOpCode {
            op: OpCode::Binary(Binary {
                op: AluBinary::Add,
                lhs: Slot::Register(bogus_reg),
                arg1: Slot::Register(arg),
                arg2: Slot::Register(arg),
            }),
            loc: empty_loc(),
        });
        let v = check_symbol_table_completeness(&obj);
        assert!(
            v.iter()
                .any(|x| matches!(x, Violation::UnregisteredRegister { .. })),
            "expected UnregisteredRegister, got {v:?}",
        );
    }

    #[test]
    fn unresolved_cast_length_is_detected() {
        let mut obj = identity_object();
        let arg = obj.arguments[0];
        let r2 = match obj.symtab.reg(Kind::Bits(16), empty_meta()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        obj.ops.push(LocatedOpCode {
            op: OpCode::AsBits(Cast {
                lhs: Slot::Register(r2),
                arg: Slot::Register(arg),
                len: None,
            }),
            loc: empty_loc(),
        });
        let v = check_unresolved_holes(&obj);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(matches!(v[0], Violation::UnresolvedCastLength { .. }));
    }
}
