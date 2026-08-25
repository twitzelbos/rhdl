//! Property-based test infrastructure for the RHIF specification.
//!
//! This module is the engineering counterpart to `doc/rhif-spec/`,
//! per Phase 2 of `rhif-formalization-plan.md`.  It provides the
//! plumbing that lets a test author check the spec's invariants on
//! a real RHIF program — both the corpus-derived programs (every
//! widget kernel) and the synthetic random programs.
//!
//! ## What's here
//!
//! - **Random `TypedBits` generator.** Given a `Kind`, produce a
//!   fully-defined `TypedBits` of that kind whose bit pattern is
//!   randomly sampled.  Used as input to VM execution for
//!   semantic-preservation tests.
//! - **Per-pass property runner.** Given a kernel, runs the full
//!   pipeline with checkpoints and verifies that every requested
//!   property holds at every checkpoint.
//! - **Semantic preservation oracle.** Given a kernel and a vector
//!   of typed inputs, runs the VM at every checkpoint and verifies
//!   the output is unchanged across every pass.
//! - **Lowering correctness oracle.** Given a kernel, compiles to
//!   RTL, runs both VMs on the same input, verifies bit-equal
//!   results.
//!
//! ## Cross-references
//!
//! - `doc/rhif-spec/` — the prose specification this module checks.
//! - `crates/rhdl-core/src/rhif/well_formedness.rs` — programmatic
//!   well-formedness checkers.
//! - `crates/rhdl-fpga/src/widget_*` — widget-corpus shadow tests
//!   that consume this module.

use rand::{Rng, RngCore, SeedableRng};

use crate::DigitalFn;
use crate::compiler::CompilationMode;
use crate::error::RHDLError;
use crate::rhif::object::Object;
use crate::rhif::spec::Slot;
use crate::rhif::vm::execute as rhif_execute;
use crate::rhif::well_formedness::{WellFormednessReport, check_object, check_object_universal};
use crate::types::bit_string::BitString;
use crate::types::kind::Kind;
use crate::types::typed_bits::TypedBits;

// ===========================================================
// Random TypedBits generation
// ===========================================================

/// Generate a fully-defined random `TypedBits` of the requested
/// kind.  The bit pattern is uniformly random over `{Zero, One}` —
/// no `X` bits.  For aggregate kinds (tuples, structs, arrays,
/// enums), the bit layout matches the kind's natural layout (per
/// `Kind::bits()`).
///
/// This is intentionally simple: it does not honour enum
/// discriminant constraints (the random pattern may produce a
/// discriminant that is not a valid variant tag, in which case the
/// VM treats the value as `dont_care`).  For most semantic
/// preservation tests this is fine — we only care that the same
/// bit pattern produces the same result before and after each
/// pass, regardless of whether it is "meaningful" enum-wise.
pub fn random_typed_bits<R: RngCore>(kind: Kind, rng: &mut R) -> TypedBits {
    let n = kind.bits();
    let bits: Vec<crate::bitx::BitX> = (0..n)
        .map(|_| {
            if rng.random::<bool>() {
                crate::bitx::BitX::One
            } else {
                crate::bitx::BitX::Zero
            }
        })
        .collect();
    TypedBits::new(bits, kind)
}

/// Convenience: generate `n` random `TypedBits` matching the
/// argument kinds of `obj.arguments`.  Returns the generated
/// values in `Object::arguments` order.
pub fn random_arguments<R: RngCore>(obj: &Object, rng: &mut R) -> Vec<TypedBits> {
    obj.arguments
        .iter()
        .map(|r| {
            let kind = obj.symtab[*r];
            random_typed_bits(kind, rng)
        })
        .collect()
}

/// All-zero `TypedBits` of the requested kind.  Useful for
/// "initial state" arguments when testing the first cycle of a
/// `Synchronous` widget.
pub fn zero_typed_bits(kind: Kind) -> TypedBits {
    let n = kind.bits();
    TypedBits::new(vec![crate::bitx::BitX::Zero; n], kind)
}

/// Build "structured" arguments for a `Synchronous` widget kernel
/// — `fn kernel(cr: ClockReset, i: I, q: Q) -> (O, D)`.  The
/// first arg (`cr`) and the third arg (`q`, the current state)
/// are zero-initialised; only the input `i` is randomly sampled.
///
/// This corresponds to "the kernel evaluating its first cycle
/// post-reset" — a meaningful semantic snapshot that exercises
/// the kernel's input-handling logic without triggering out-of-
/// range dynamic-index lookups against random `q` state.
///
/// For lowering-correctness tests on widgets whose internal `q`
/// state has dynamic-index reads (FIFOs, register files, protocol
/// PHYs with byte buffers), this is the right discipline:
/// fully-random `q` produces ICE rates of nearly 100 %, while
/// zero-`q` evaluates the kernel's normal first-cycle behaviour.
pub fn structured_synchronous_arguments<R: RngCore>(obj: &Object, rng: &mut R) -> Vec<TypedBits> {
    obj.arguments
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let kind = obj.symtab[*r];
            // Convention: arg 0 = cr, arg 1 = i, arg 2 = q.
            // Randomise the input; zero the rest.
            if i == 1 {
                random_typed_bits(kind, rng)
            } else {
                zero_typed_bits(kind)
            }
        })
        .collect()
}

// ===========================================================
// Per-pass property runner
// ===========================================================

/// Outcome of running the per-pass property suite on one kernel.
#[derive(Debug)]
pub struct PerPassReport {
    /// `(pass_name, post_pass_well_formedness_report)` for every
    /// pass invocation in the pipeline.  Empty if the compile failed
    /// before any pass ran.
    pub checkpoints: Vec<(String, WellFormednessReport)>,
}

impl PerPassReport {
    /// True iff every checkpoint produced a well-formed Object.
    #[must_use]
    pub fn all_well_formed(&self) -> bool {
        self.checkpoints.iter().all(|(_, r)| r.is_well_formed())
    }

    /// The first non-well-formed checkpoint, if any.
    pub fn first_violation(&self) -> Option<&(String, WellFormednessReport)> {
        self.checkpoints.iter().find(|(_, r)| !r.is_well_formed())
    }
}

/// Run a kernel through the full pipeline with checkpoints, and at
/// every checkpoint verify the Object is well-formed under the
/// **universal** invariants — those that hold at every checkpoint
/// of the stage1 pipeline (per `doc/rhif-spec/invariants/passes.md`).
///
/// At the *final* checkpoint (the post-pipeline Object), this also
/// runs the late-stage invariant set ("unresolved holes" — every
/// `Cast` has its length resolved, every `Retime` has its colour
/// resolved, every `Wrap` has its kind resolved).
///
/// Returns a `PerPassReport` enumerating each pass's outcome.
pub fn run_per_pass_well_formedness<K: DigitalFn>(
    mode: CompilationMode,
) -> Result<PerPassReport, RHDLError> {
    use std::cell::RefCell;
    let report = RefCell::new(PerPassReport {
        checkpoints: Vec::new(),
    });
    let mut hook = |pass: &'static str, obj: &Object| -> Result<(), RHDLError> {
        let r = check_object_universal(obj);
        report.borrow_mut().checkpoints.push((pass.to_string(), r));
        Ok(())
    };
    let final_obj =
        crate::compiler::driver::compile_design_stage1_with_checkpoints::<K>(mode, &mut hook)?;
    // Replace the last universal-only checkpoint with a full check on
    // the final Object — at that point the late-stage invariants
    // ("unresolved holes") are also expected to hold.
    let mut report = report.into_inner();
    let full_final = check_object(&final_obj);
    if let Some(last) = report.checkpoints.last_mut() {
        *last = (last.0.clone(), full_final);
    }
    Ok(report)
}

// ===========================================================
// Semantic preservation oracle
// ===========================================================

/// One execution outcome of running the VM on a particular Object.
/// Wraps both successful results and errors so the comparison can
/// treat "both errored with the same message" as observation-
/// equivalent — important for kernels that have out-of-domain
/// inputs (e.g., dynamic index into an array, where the VM panics
/// on out-of-range indices regardless of which pass produced the
/// Object).
#[derive(Debug, Clone, PartialEq)]
enum VmOutcome {
    Ok(TypedBits),
    Err(String),
}

/// Outcome of a semantic-preservation run.  Records a divergence
/// (the first observed pass where the VM outcome differs from the
/// reference) or success across every checkpoint.
#[derive(Debug)]
pub enum SemanticPreservationOutcome {
    /// Every checkpoint produced the same outcome as the initial
    /// (post-`infer`) Object — same output, or same error.
    Preserved { checkpoints: usize },
    /// At one checkpoint, the VM produced a different outcome than
    /// the reference.  Either the output bits differ, or one side
    /// errored when the other succeeded, or both errored but with
    /// different messages.
    Diverged {
        pass_name: String,
        checkpoint_index: usize,
        reference: String,
        observed: String,
    },
}

impl SemanticPreservationOutcome {
    #[must_use]
    pub fn is_preserved(&self) -> bool {
        matches!(self, SemanticPreservationOutcome::Preserved { .. })
    }
}

fn outcome_repr(o: &VmOutcome) -> String {
    match o {
        VmOutcome::Ok(v) => format!("Ok({:?})", v.bits()),
        VmOutcome::Err(e) => format!("Err({e})"),
    }
}

/// Compile a kernel with checkpoints, run the VM at each checkpoint
/// with the supplied arguments, and verify every outcome (success
/// or error) is identical to the initial (post-`infer`) Object's.
///
/// This is the strongest property the plan calls for: every pass is
/// observation-equivalent — the outcome of running the VM on a given
/// input must be the same before and after every pass.  "Outcome"
/// includes the case where the VM errors (e.g., out-of-range
/// dynamic index): if both checkpoints error with the same message,
/// they are observation-equivalent.
pub fn check_semantic_preservation<K: DigitalFn>(
    mode: CompilationMode,
    arguments: Vec<TypedBits>,
) -> Result<SemanticPreservationOutcome, RHDLError> {
    use std::cell::RefCell;
    let reference: RefCell<Option<VmOutcome>> = RefCell::new(None);
    let outcome: RefCell<Option<SemanticPreservationOutcome>> = RefCell::new(None);
    let counter: RefCell<usize> = RefCell::new(0);

    let mut hook = |pass: &'static str, obj: &Object| -> Result<(), RHDLError> {
        if outcome.borrow().is_some() {
            *counter.borrow_mut() += 1;
            return Ok(());
        }
        let observed = match rhif_execute(obj, arguments.clone()) {
            Ok(v) => VmOutcome::Ok(v),
            Err(e) => VmOutcome::Err(format!("{e:?}")),
        };
        let mut r = reference.borrow_mut();
        match r.as_ref() {
            None => {
                // No reference yet.  Skip Err outcomes — the VM is
                // documented as defined only after the lowering
                // passes resolve `len` / `color` / `kind` holes.
                // Pin the first Ok as the reference; from then on,
                // every later outcome must match.
                if matches!(observed, VmOutcome::Ok(_)) {
                    *r = Some(observed);
                }
            }
            Some(reference) => {
                let equiv = match (reference, &observed) {
                    (VmOutcome::Ok(a), VmOutcome::Ok(b)) => {
                        a.bits() == b.bits() && a.kind() == b.kind()
                    }
                    (VmOutcome::Err(a), VmOutcome::Err(b)) => a == b,
                    _ => false,
                };
                if !equiv {
                    *outcome.borrow_mut() = Some(SemanticPreservationOutcome::Diverged {
                        pass_name: pass.to_string(),
                        checkpoint_index: *counter.borrow(),
                        reference: outcome_repr(reference),
                        observed: outcome_repr(&observed),
                    });
                }
            }
        }
        *counter.borrow_mut() += 1;
        Ok(())
    };

    crate::compiler::driver::compile_design_stage1_with_checkpoints::<K>(mode, &mut hook)?;

    if let Some(o) = outcome.into_inner() {
        Ok(o)
    } else {
        Ok(SemanticPreservationOutcome::Preserved {
            checkpoints: counter.into_inner(),
        })
    }
}

// ===========================================================
// Lowering correctness oracle
// ===========================================================

/// Outcome of running a kernel through both the RHIF VM and the RTL
/// VM with the same arguments.
#[derive(Debug)]
pub enum LoweringCorrectnessOutcome {
    /// Both VMs produced the same bit pattern.
    Equal,
    /// Bit-pattern mismatch.
    Mismatch {
        rhif_bits: Vec<crate::bitx::BitX>,
        rtl_bits: Vec<crate::bitx::BitX>,
    },
    /// One side errored; the other was not run.
    RhifError {
        error: String,
    },
    RtlError {
        error: String,
    },
}

impl LoweringCorrectnessOutcome {
    #[must_use]
    pub fn is_equal(&self) -> bool {
        matches!(self, LoweringCorrectnessOutcome::Equal)
    }
}

/// Compile a kernel through stage1 + stage2, run both VMs with the
/// same arguments, compare outputs.  This is the lowering-correctness
/// property: RHIF → RTL is observation-equivalent on any input.
pub fn check_lowering_correctness<K: DigitalFn>(
    mode: CompilationMode,
    arguments: Vec<TypedBits>,
) -> Result<LoweringCorrectnessOutcome, RHDLError> {
    let rhif_obj = crate::compiler::driver::compile_design_stage1::<K>(mode)?;
    let rhif_result = match rhif_execute(&rhif_obj, arguments.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(LoweringCorrectnessOutcome::RhifError {
                error: format!("{e:?}"),
            });
        }
    };

    let rtl_obj = crate::compiler::driver::compile_design_stage2(&rhif_obj)?;
    let rtl_args: Vec<BitString> = arguments.iter().map(BitString::from).collect();
    let rtl_result = match crate::rtl::vm::execute(&rtl_obj, rtl_args) {
        Ok(v) => v,
        Err(e) => {
            return Ok(LoweringCorrectnessOutcome::RtlError {
                error: format!("{e:?}"),
            });
        }
    };

    let rhif_bits: Vec<crate::bitx::BitX> = rhif_result.bits().to_vec();
    let rtl_bits: Vec<crate::bitx::BitX> = rtl_result.bits().to_vec();
    if rhif_bits == rtl_bits {
        Ok(LoweringCorrectnessOutcome::Equal)
    } else {
        Ok(LoweringCorrectnessOutcome::Mismatch {
            rhif_bits,
            rtl_bits,
        })
    }
}

// ===========================================================
// Convenience: deterministic seed
// ===========================================================

/// Build a deterministically-seeded RNG.  Tests should pin a seed
/// so they are reproducible, but use one seed value site-by-site
/// so a regression that affects only one widget surfaces clearly.
#[must_use]
pub fn seeded_rng(seed: u64) -> rand::rngs::StdRng {
    rand::rngs::StdRng::seed_from_u64(seed)
}

// ===========================================================
// Minimal random-RHIF-program generator
// ===========================================================

/// Build a minimal valid `SymbolMap` for a synthetic Object — one
/// that has a single `SpannedSource` entry covering the supplied
/// `function_id`, with a fallback `NodeId(0)` and an empty source.
///
/// This is enough to satisfy the VM's `symbols.fallback(fn_id)`
/// lookup, which would otherwise panic on synthetic Objects whose
/// SymbolMap is the default-empty.  Pair with [`generate_chain_program`]
/// to produce VM-runnable random programs.
pub fn synthetic_symbol_map(
    function_id: crate::ast::ast_impl::FunctionId,
) -> crate::rhif::object::SymbolMap {
    use crate::ast::ast_impl::NodeId;
    use crate::ast::spanned_source::{SpannedSource, SpannedSourceSet};

    let mut span_map = std::collections::HashMap::new();
    span_map.insert(NodeId::new(0), 0..0);
    let source = SpannedSource {
        source: String::new(),
        name: "synthetic".to_string(),
        span_map,
        fallback: NodeId::new(0),
        filename: "<synthetic>".to_string(),
        function_id,
    };
    let source_set = SpannedSourceSet::from((function_id, source));
    crate::rhif::object::SymbolMap { source_set }
}

/// A random program is a single-argument, single-return RHIF
/// `Object` whose body is a chain of width-preserving binary ops
/// from the argument to the return.  This is the simplest non-
/// trivial program shape; it exercises `Binary`, `Unary`, `Assign`,
/// and `Select` against both a register and a literal operand
/// pool.
///
/// Per `rhif-formalization-plan.md` §5.2, this corresponds to the
/// "synthetic random programs" leg of Phase 2's two-corpus
/// strategy.  The other leg is the widget corpus, which exercises
/// the full opcode surface but not corner-case shapes.
///
/// More elaborate generators (covering `Index`, `Splice`,
/// `Tuple`/`Array`/`Struct`/`Enum` aggregates, `Case` dispatch,
/// `Exec` calls) are deferred — type-correct random program
/// generation across all 19 opcodes is a longer-tail effort
/// per the plan.
pub fn generate_chain_program(bit_width: usize, n_ops: usize, rng: &mut impl RngCore) -> Object {
    use crate::TypedBits;
    use crate::ast::SourceLocation;
    use crate::ast::ast_impl::FunctionId;
    use crate::common::symtab::SymbolTable;
    use crate::rhif::object::{LocatedOpCode, SourceDetails};
    use crate::rhif::rhif_builder::{op_binary, op_unary};
    use crate::rhif::spec::{AluBinary, AluUnary, SlotKind};

    let kind = Kind::Bits(bit_width);
    let fid = FunctionId::from(0u64);
    let loc = SourceLocation {
        node: crate::ast::ast_impl::NodeId::new(0),
        func: fid,
    };
    let meta = SourceDetails {
        location: loc,
        name: None,
    };

    let mut symtab: SymbolTable<TypedBits, Kind, SourceDetails, SlotKind> = SymbolTable::default();
    let arg = match symtab.reg(kind, meta.clone()) {
        Slot::Register(r) => r,
        _ => unreachable!(),
    };
    let mut current = Slot::Register(arg);
    let mut ops: Vec<LocatedOpCode> = Vec::with_capacity(n_ops);

    let binops = [
        AluBinary::Add,
        AluBinary::Sub,
        AluBinary::BitXor,
        AluBinary::BitAnd,
        AluBinary::BitOr,
    ];
    let unaryops = [AluUnary::Not];

    for _ in 0..n_ops {
        let lhs = match symtab.reg(kind, meta.clone()) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        let lhs_slot = Slot::Register(lhs);
        // 50/50: binary with a constant literal operand vs. unary.
        if rng.random::<bool>() {
            let rhs_lit = symtab.lit(random_typed_bits(kind, rng), meta.clone());
            let op = binops[(rng.next_u32() as usize) % binops.len()];
            ops.push(LocatedOpCode {
                op: op_binary(op, lhs_slot, current, rhs_lit),
                loc,
            });
        } else {
            let op = unaryops[(rng.next_u32() as usize) % unaryops.len()];
            ops.push(LocatedOpCode {
                op: op_unary(op, lhs_slot, current),
                loc,
            });
        }
        current = lhs_slot;
    }

    Object {
        symbols: synthetic_symbol_map(fid),
        symtab,
        return_slot: current,
        externals: Default::default(),
        ops,
        arguments: vec![arg],
        name: format!("random_chain_{bit_width}b_{n_ops}ops"),
        fn_id: fid,
        flags: Vec::new(),
    }
}

/// Internal helper used by every random-program generator.  Owns
/// the `SymbolTable` and provides an interning helper for slots.
#[allow(dead_code)]
struct ProgramBuilder {
    symtab: crate::common::symtab::SymbolTable<
        TypedBits,
        Kind,
        crate::rhif::object::SourceDetails,
        crate::rhif::spec::SlotKind,
    >,
    ops: Vec<crate::rhif::object::LocatedOpCode>,
    arg: crate::common::symtab::RegisterId<crate::rhif::spec::SlotKind>,
    arg_kind: Kind,
    fid: crate::ast::ast_impl::FunctionId,
    loc: crate::ast::SourceLocation,
}

impl ProgramBuilder {
    fn new(arg_kind: Kind) -> Self {
        use crate::ast::SourceLocation;
        use crate::ast::ast_impl::{FunctionId, NodeId};
        use crate::common::symtab::SymbolTable;
        use crate::rhif::object::SourceDetails;
        use crate::rhif::spec::Slot;
        let fid = FunctionId::from(0u64);
        let loc = SourceLocation {
            node: NodeId::new(0),
            func: fid,
        };
        let meta = SourceDetails {
            location: loc,
            name: None,
        };
        let mut symtab: SymbolTable<TypedBits, Kind, SourceDetails, _> = SymbolTable::default();
        let arg = match symtab.reg(arg_kind, meta) {
            Slot::Register(r) => r,
            _ => unreachable!(),
        };
        Self {
            symtab,
            ops: Vec::new(),
            arg,
            arg_kind,
            fid,
            loc,
        }
    }

    fn meta(&self) -> crate::rhif::object::SourceDetails {
        crate::rhif::object::SourceDetails {
            location: self.loc,
            name: None,
        }
    }

    fn new_register(
        &mut self,
        kind: Kind,
    ) -> crate::common::symtab::RegisterId<crate::rhif::spec::SlotKind> {
        let meta = self.meta();
        match self.symtab.reg(kind, meta) {
            crate::rhif::spec::Slot::Register(r) => r,
            _ => unreachable!(),
        }
    }

    fn new_literal(&mut self, value: TypedBits) -> crate::rhif::spec::Slot {
        let meta = self.meta();
        self.symtab.lit(value, meta)
    }

    fn push(&mut self, op: crate::rhif::spec::OpCode) {
        self.ops
            .push(crate::rhif::object::LocatedOpCode { op, loc: self.loc });
    }

    fn finish(self, return_slot: crate::rhif::spec::Slot, name: &str) -> Object {
        Object {
            symbols: synthetic_symbol_map(self.fid),
            symtab: self.symtab,
            return_slot,
            externals: Default::default(),
            ops: self.ops,
            arguments: vec![self.arg],
            name: name.to_string(),
            fn_id: self.fid,
            flags: Vec::new(),
        }
    }
}

/// Generate a program that builds a tuple from the argument and a
/// literal, then indexes back out one of the components.
///
/// Shape: `tuple = (arg, literal); return tuple.0` (or `.1`).
/// Exercises `Tuple` + `Index` (via `TupleIndex`).
pub fn generate_tuple_program(bit_width: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::{op_index, op_tuple};
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;

    let kind = Kind::Bits(bit_width);
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let lit_slot = b.new_literal(random_typed_bits(kind, rng));
    let tuple_kind = Kind::make_tuple(vec![kind, kind].into());
    let tuple_reg = b.new_register(tuple_kind);
    let tuple_slot = Slot::Register(tuple_reg);
    b.push(op_tuple(tuple_slot, vec![arg_slot, lit_slot]));
    let result_reg = b.new_register(kind);
    let result_slot = Slot::Register(result_reg);
    let pick = if rng.random::<bool>() { 0 } else { 1 };
    b.push(op_index(
        result_slot,
        tuple_slot,
        Path::default().tuple_index(pick),
    ));
    b.finish(result_slot, &format!("random_tuple_{bit_width}b"))
}

/// Generate a program that builds an array from the argument
/// repeated, then indexes a constant element.
///
/// Shape: `arr = [arg, lit, arg, lit]; return arr[k]`.
/// Exercises `Array` + `Index` (constant index).
pub fn generate_array_program(bit_width: usize, n_elem: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::{op_array, op_index};
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;

    assert!(n_elem >= 1);
    let kind = Kind::Bits(bit_width);
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let elements: Vec<Slot> = (0..n_elem)
        .map(|i| {
            if i == 0 || rng.random::<bool>() {
                arg_slot
            } else {
                b.new_literal(random_typed_bits(kind, rng))
            }
        })
        .collect();
    let arr_kind = Kind::make_array(kind, n_elem);
    let arr_reg = b.new_register(arr_kind);
    let arr_slot = Slot::Register(arr_reg);
    b.push(op_array(arr_slot, elements));
    let pick = (rng.next_u32() as usize) % n_elem;
    let out_reg = b.new_register(kind);
    let out_slot = Slot::Register(out_reg);
    b.push(op_index(out_slot, arr_slot, Path::default().index(pick)));
    b.finish(out_slot, &format!("random_array_{bit_width}b_{n_elem}e"))
}

/// Generate a program that uses `Select` to pick between two
/// computed values based on a 1-bit literal condition.
///
/// Shape: `cond = lit; a = arg + lit; b = arg ^ lit;
/// return select(cond, a, b)`.  Exercises `Binary` + `Select`.
pub fn generate_select_program(bit_width: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::{op_binary, op_select};
    use crate::rhif::spec::{AluBinary, Slot};

    let kind = Kind::Bits(bit_width);
    let bool_kind = Kind::Bits(1);
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let lit_a = b.new_literal(random_typed_bits(kind, rng));
    let lit_b = b.new_literal(random_typed_bits(kind, rng));
    let cond_lit = b.new_literal(random_typed_bits(bool_kind, rng));

    let a_reg = b.new_register(kind);
    let a_slot = Slot::Register(a_reg);
    b.push(op_binary(AluBinary::Add, a_slot, arg_slot, lit_a));

    let bv_reg = b.new_register(kind);
    let bv_slot = Slot::Register(bv_reg);
    b.push(op_binary(AluBinary::BitXor, bv_slot, arg_slot, lit_b));

    let result_reg = b.new_register(kind);
    let result_slot = Slot::Register(result_reg);
    b.push(op_select(result_slot, cond_lit, a_slot, bv_slot));
    b.finish(result_slot, &format!("random_select_{bit_width}b"))
}

/// Generate a program that builds an array via `Repeat` and indexes
/// it.  Shape: `arr = [arg; N]; return arr[k]`.  Exercises `Repeat`
/// + `Index`.
pub fn generate_repeat_program(bit_width: usize, n_elem: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::{op_index, op_repeat};
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;

    assert!(n_elem >= 1);
    let kind = Kind::Bits(bit_width);
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let arr_kind = Kind::make_array(kind, n_elem);
    let arr_reg = b.new_register(arr_kind);
    let arr_slot = Slot::Register(arr_reg);
    b.push(op_repeat(arr_slot, arg_slot, n_elem as u64));
    let pick = (rng.next_u32() as usize) % n_elem;
    let out_reg = b.new_register(kind);
    let out_slot = Slot::Register(out_reg);
    b.push(op_index(out_slot, arr_slot, Path::default().index(pick)));
    b.finish(out_slot, &format!("random_repeat_{bit_width}b_{n_elem}e"))
}

/// Generate a program that builds a tuple, splices a fresh value
/// into one component, then indexes back out.
///
/// Shape: `t0 = (arg, lit); t1 = t0[0 := lit2]; return t1.0`.
/// Exercises `Tuple` + `Splice` + `Index`.
pub fn generate_splice_program(bit_width: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::{op_index, op_splice, op_tuple};
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;

    let kind = Kind::Bits(bit_width);
    let tuple_kind = Kind::make_tuple(vec![kind, kind].into());
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let lit1 = b.new_literal(random_typed_bits(kind, rng));
    let lit2 = b.new_literal(random_typed_bits(kind, rng));

    let t0_reg = b.new_register(tuple_kind);
    let t0_slot = Slot::Register(t0_reg);
    b.push(op_tuple(t0_slot, vec![arg_slot, lit1]));

    let t1_reg = b.new_register(tuple_kind);
    let t1_slot = Slot::Register(t1_reg);
    b.push(op_splice(
        t1_slot,
        t0_slot,
        Path::default().tuple_index(0),
        lit2,
    ));

    let out_reg = b.new_register(kind);
    let out_slot = Slot::Register(out_reg);
    b.push(op_index(out_slot, t1_slot, Path::default().tuple_index(0)));
    b.finish(out_slot, &format!("random_splice_{bit_width}b"))
}

/// Generate a width-changing cast chain.  Shape:
/// `r1 = arg.resize::<m>(); r2 = r1.resize::<n>(); return r2`.
/// Exercises `Resize`.
pub fn generate_cast_program(
    in_width: usize,
    mid_width: usize,
    out_width: usize,
    _rng: &mut impl RngCore,
) -> Object {
    use crate::rhif::rhif_builder::op_resize;
    use crate::rhif::spec::Slot;

    let in_kind = Kind::Bits(in_width);
    let mid_kind = Kind::Bits(mid_width);
    let out_kind = Kind::Bits(out_width);
    let mut b = ProgramBuilder::new(in_kind);
    let arg_slot = Slot::Register(b.arg);

    let r1_reg = b.new_register(mid_kind);
    let r1_slot = Slot::Register(r1_reg);
    b.push(op_resize(r1_slot, arg_slot, mid_width));

    let r2_reg = b.new_register(out_kind);
    let r2_slot = Slot::Register(r2_reg);
    b.push(op_resize(r2_slot, r1_slot, out_width));
    b.finish(r2_slot, &format!("random_cast_{in_width}b_to_{out_width}b"))
}

/// Build the `Option<payload>` kind in the shape RHDL's `wrap_some`
/// / `wrap_none` helpers expect: 2 variants ("None", "Some") with
/// the `Some` variant carrying a single-element tuple of `payload`,
/// and a 1-bit MSB-aligned unsigned discriminant.
fn build_option_kind(payload: Kind) -> Kind {
    use crate::types::kind::{DiscriminantAlignment, DiscriminantType};
    Kind::make_enum(
        &format!("Option::<{payload:?}>"),
        vec![
            Kind::make_variant("None", Kind::Empty, 0),
            Kind::make_variant("Some", Kind::make_tuple(vec![payload].into()), 1),
        ],
        Kind::make_discriminant_layout(1, DiscriminantAlignment::Msb, DiscriminantType::Unsigned),
    )
}

/// Build the `Result<ok, err>` kind in the shape RHDL's `wrap_ok`
/// / `wrap_err` helpers expect.  Currently unused by the public
/// generators but kept for parity with [`build_option_kind`] —
/// future `wrap_ok` / `wrap_err` generators will use it.
#[allow(dead_code)]
fn build_result_kind(ok: Kind, err: Kind) -> Kind {
    use crate::types::kind::{DiscriminantAlignment, DiscriminantType};
    Kind::make_enum(
        &format!("Result::<{ok:?}, {err:?}>"),
        vec![
            Kind::make_variant("Ok", Kind::make_tuple(vec![ok].into()), 0),
            Kind::make_variant("Err", Kind::make_tuple(vec![err].into()), 1),
        ],
        Kind::make_discriminant_layout(1, DiscriminantAlignment::Msb, DiscriminantType::Unsigned),
    )
}

/// Generate a program that casts the argument to a different
/// unsigned width via `AsBits`.  Shape: `r = arg as Bits<n>`.
pub fn generate_as_bits_program(in_width: usize, out_width: usize) -> Object {
    use crate::rhif::rhif_builder::op_as_bits;
    use crate::rhif::spec::Slot;
    let in_kind = Kind::Bits(in_width);
    let out_kind = Kind::Bits(out_width);
    let mut b = ProgramBuilder::new(in_kind);
    let arg_slot = Slot::Register(b.arg);
    let r = b.new_register(out_kind);
    let r_slot = Slot::Register(r);
    b.push(op_as_bits(r_slot, arg_slot, out_width));
    b.finish(
        r_slot,
        &format!("random_as_bits_{in_width}b_to_{out_width}b"),
    )
}

/// Generate a program that reinterprets the argument as signed via
/// `AsSigned`.  Shape: `r = arg as SignedBits<n>`.
pub fn generate_as_signed_program(in_width: usize, out_width: usize) -> Object {
    use crate::rhif::rhif_builder::op_as_signed;
    use crate::rhif::spec::Slot;
    let in_kind = Kind::Bits(in_width);
    let out_kind = Kind::Signed(out_width);
    let mut b = ProgramBuilder::new(in_kind);
    let arg_slot = Slot::Register(b.arg);
    let r = b.new_register(out_kind);
    let r_slot = Slot::Register(r);
    b.push(op_as_signed(r_slot, arg_slot, out_width));
    b.finish(
        r_slot,
        &format!("random_as_signed_{in_width}b_to_{out_width}b"),
    )
}

/// Generate a program that wraps the argument in a `Signal<T, C>`
/// via `Retime`, then strips the wrapper via `Index(SignalValue)`
/// and returns the inner value.  Exercises `Retime` + signal-aware
/// `Index`.
pub fn generate_retime_program(bit_width: usize) -> Object {
    use crate::Color;
    use crate::rhif::rhif_builder::{op_index, op_retime};
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;
    let inner_kind = Kind::Bits(bit_width);
    let signal_kind = Kind::make_signal(inner_kind, Color::Red);
    let mut b = ProgramBuilder::new(inner_kind);
    let arg_slot = Slot::Register(b.arg);
    let signal_reg = b.new_register(signal_kind);
    let signal_slot = Slot::Register(signal_reg);
    b.push(op_retime(signal_slot, arg_slot, Some(Color::Red)));
    let out_reg = b.new_register(inner_kind);
    let out_slot = Slot::Register(out_reg);
    b.push(op_index(
        out_slot,
        signal_slot,
        Path::default().signal_value(),
    ));
    b.finish(out_slot, &format!("random_retime_{bit_width}b"))
}

/// Generate a program that wraps the argument in `Some(_)` via
/// `Wrap(Some)`, then extracts the discriminant via `Index` and
/// returns it as a `Bits(1)`.  Exercises `Wrap` + enum
/// discriminant `Index`.
pub fn generate_wrap_some_program(bit_width: usize) -> Object {
    use crate::ast::ast_impl::WrapOp;
    use crate::rhif::rhif_builder::op_index;
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;
    let payload_kind = Kind::Bits(bit_width);
    let option_kind = build_option_kind(payload_kind);
    let mut b = ProgramBuilder::new(payload_kind);
    let arg_slot = Slot::Register(b.arg);
    let opt_reg = b.new_register(option_kind);
    let opt_slot = Slot::Register(opt_reg);
    b.push(crate::rhif::spec::OpCode::Wrap(crate::rhif::spec::Wrap {
        op: WrapOp::Some,
        lhs: opt_slot,
        arg: arg_slot,
        kind: Some(option_kind),
    }));
    let disc_reg = b.new_register(Kind::Bits(1));
    let disc_slot = Slot::Register(disc_reg);
    b.push(op_index(
        disc_slot,
        opt_slot,
        Path::default().discriminant(),
    ));
    b.finish(disc_slot, &format!("random_wrap_some_{bit_width}b"))
}

/// Generate a program that builds an `Option::None` of the
/// requested payload kind, returns its discriminant as `Bits(1)`.
pub fn generate_wrap_none_program(payload_bit_width: usize) -> Object {
    use crate::TypedBits;
    use crate::ast::ast_impl::WrapOp;
    use crate::rhif::rhif_builder::{op_assign, op_index};
    use crate::rhif::spec::Slot;
    use crate::types::path::Path;
    let payload_kind = Kind::Bits(payload_bit_width);
    let option_kind = build_option_kind(payload_kind);

    // The argument kind is `Empty` because `wrap_none` requires the
    // arg to be of the None payload's kind (which is Empty).  But we
    // want the kernel to take a "real" argument of the payload kind
    // for VM input, so we ignore the argument and feed an Empty
    // literal into the wrap.
    let mut b = ProgramBuilder::new(payload_kind);
    // Force a use of the arg via Assign-into-discard register.
    let _arg_slot = Slot::Register(b.arg);
    let empty_lit = b.new_literal(TypedBits::new(vec![], Kind::Empty));

    let opt_reg = b.new_register(option_kind);
    let opt_slot = Slot::Register(opt_reg);
    b.push(crate::rhif::spec::OpCode::Wrap(crate::rhif::spec::Wrap {
        op: WrapOp::None,
        lhs: opt_slot,
        arg: empty_lit,
        kind: Some(option_kind),
    }));

    let disc_reg = b.new_register(Kind::Bits(1));
    let disc_slot = Slot::Register(disc_reg);
    b.push(op_index(
        disc_slot,
        opt_slot,
        Path::default().discriminant(),
    ));

    // To keep the argument referenced (so it doesn't trigger
    // RemoveUnusedRegisters), assign it into a discard register.
    let _ = op_assign;
    b.finish(disc_slot, &format!("random_wrap_none_{payload_bit_width}b"))
}

/// Generate a program that builds a 2-field struct from the
/// argument and a literal, then indexes the named field back out.
pub fn generate_struct_program(bit_width: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::{op_index, op_struct};
    use crate::rhif::spec::{FieldValue, Member, Slot};
    use crate::types::path::Path;
    use internment::Intern;

    let kind = Kind::Bits(bit_width);
    let struct_kind = Kind::make_struct(
        "RandStruct",
        vec![Kind::make_field("a", kind), Kind::make_field("b", kind)].into(),
    );

    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let lit = b.new_literal(random_typed_bits(kind, rng));
    // Build a "template" TypedBits — all-zero of the struct kind.
    let template = zero_typed_bits(struct_kind);
    let s_reg = b.new_register(struct_kind);
    let s_slot = Slot::Register(s_reg);
    b.push(op_struct(
        s_slot,
        vec![
            FieldValue {
                member: Member::Named(Intern::new("a".to_string())),
                value: arg_slot,
            },
            FieldValue {
                member: Member::Named(Intern::new("b".to_string())),
                value: lit,
            },
        ],
        None,
        template,
    ));

    let out_reg = b.new_register(kind);
    let out_slot = Slot::Register(out_reg);
    b.push(op_index(out_slot, s_slot, Path::default().field("a")));
    b.finish(out_slot, &format!("random_struct_{bit_width}b"))
}

/// Generate a program that constructs a specific variant of a
/// 2-variant enum with the argument as the variant's payload, then
/// extracts the discriminant.  Exercises `Enum` + enum-discriminant
/// `Index`.  The chosen variant is `B` (discriminant 1), whose
/// payload is `(arg,)` — a single-element tuple.
pub fn generate_enum_program(bit_width: usize) -> Object {
    use crate::rhif::rhif_builder::op_index;
    use crate::rhif::spec::{FieldValue, Member, Slot};
    use crate::types::kind::{DiscriminantAlignment, DiscriminantType};
    use crate::types::path::Path;

    let payload_kind = Kind::Bits(bit_width);
    let enum_kind = Kind::make_enum(
        "RandEnum",
        vec![
            Kind::make_variant("A", Kind::Empty, 0),
            Kind::make_variant("B", Kind::make_tuple(vec![payload_kind].into()), 1),
        ],
        Kind::make_discriminant_layout(1, DiscriminantAlignment::Msb, DiscriminantType::Unsigned),
    );

    let mut b = ProgramBuilder::new(payload_kind);
    let arg_slot = Slot::Register(b.arg);

    // Template: discriminant = 1 (variant B), payload = zero.
    // Bit layout per `is_option` style: discriminant is MSB-aligned
    // 1 bit, then the payload.  For an Msb-aligned 1-bit
    // discriminant + N-bit payload, total bits are 1+N with the
    // discriminant in position [N].  See `Kind::pad`.
    let mut bits = vec![crate::bitx::BitX::Zero; enum_kind.bits()];
    // Discriminant bit position: MSB (last bit in the vector).
    *bits.last_mut().unwrap() = crate::bitx::BitX::One;
    let template = TypedBits::new(bits, enum_kind);

    let e_reg = b.new_register(enum_kind);
    let e_slot = Slot::Register(e_reg);
    b.push(crate::rhif::spec::OpCode::Enum(crate::rhif::spec::Enum {
        lhs: e_slot,
        fields: vec![FieldValue {
            member: Member::Unnamed(0),
            value: arg_slot,
        }],
        template,
    }));

    let disc_reg = b.new_register(Kind::Bits(1));
    let disc_slot = Slot::Register(disc_reg);
    b.push(op_index(disc_slot, e_slot, Path::default().discriminant()));
    b.finish(disc_slot, &format!("random_enum_{bit_width}b"))
}

/// Generate a program that calls a synthetic callee via `Exec` and
/// returns the call's result.  The callee is a 1-arg, 1-return
/// chain program (`generate_chain_program(bit_width, 2, _)`).
pub fn generate_exec_program(bit_width: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::op_exec;
    use crate::rhif::spec::{FuncId, Slot};

    let kind = Kind::Bits(bit_width);
    let callee = generate_chain_program(bit_width, 2, rng);
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let r = b.new_register(kind);
    let r_slot = Slot::Register(r);
    let func_id = FuncId::from(0usize);
    b.push(op_exec(r_slot, func_id, vec![arg_slot]));

    let mut obj = b.finish(r_slot, &format!("random_exec_{bit_width}b"));
    obj.externals.insert(func_id, Box::new(callee));
    obj
}

/// Generate a program that uses `Case` to multi-way-select between
/// three values based on a literal discriminator.  Exercises `Case`
/// with both `Slot` and `Wild` arms.
pub fn generate_case_program(bit_width: usize, rng: &mut impl RngCore) -> Object {
    use crate::rhif::rhif_builder::op_case;
    use crate::rhif::spec::{CaseArgument, Slot};

    let kind = Kind::Bits(bit_width);
    let mut b = ProgramBuilder::new(kind);
    let arg_slot = Slot::Register(b.arg);
    let disc_lit = b.new_literal(random_typed_bits(kind, rng));
    let arm_a = b.new_literal(random_typed_bits(kind, rng));
    let arm_b = b.new_literal(random_typed_bits(kind, rng));

    let arm_disc_a = b.new_literal(random_typed_bits(kind, rng));
    let arm_disc_b = b.new_literal(random_typed_bits(kind, rng));

    let out_reg = b.new_register(kind);
    let out_slot = Slot::Register(out_reg);
    b.push(op_case(
        out_slot,
        disc_lit,
        vec![
            (CaseArgument::Slot(arm_disc_a), arm_a),
            (CaseArgument::Slot(arm_disc_b), arm_b),
            (CaseArgument::Wild, arg_slot),
        ],
    ));
    b.finish(out_slot, &format!("random_case_{bit_width}b"))
}

/// Run a sequence of mutating passes against `obj`, checking
/// well-formedness after each one.  Returns the post-pipeline
/// `Object` (or the first violation encountered).
///
/// The passes here are a subset of the stage1 pipeline: those
/// whose `Pass::run` accepts any well-formed `Object`, regardless
/// of how it was constructed (some passes assume specific MIR-
/// derived structure and aren't applicable to synthetic programs).
pub fn run_passes_on_random_program(obj: Object) -> Result<Object, (String, WellFormednessReport)> {
    use crate::compiler::rhif_passes::pass::Pass;
    use crate::compiler::rhif_passes::{
        constant_propagation::ConstantPropagation, dead_code_elimination::DeadCodeEliminationPass,
        propagate_literals::PropagateLiteralsPass,
        remove_extra_registers::RemoveExtraRegistersPass,
        remove_unneeded_muxes::RemoveUnneededMuxesPass,
        remove_unused_literals::RemoveUnusedLiterals,
        remove_unused_registers::RemoveUnusedRegistersPass,
    };

    macro_rules! step {
        ($obj:expr, $pass:ty) => {{
            let post = match <$pass>::run($obj) {
                Ok(o) => o,
                Err(e) => return Err((<$pass>::description().to_string(), error_report(e))),
            };
            let r = check_object_universal(&post);
            if !r.is_well_formed() {
                return Err((<$pass>::description().to_string(), r));
            }
            post
        }};
    }

    fn error_report(_e: RHDLError) -> WellFormednessReport {
        // We don't have a Violation variant for "pass errored"; treat
        // it as an empty violation set with a synthesised description.
        // The caller distinguishes by checking is_well_formed().
        WellFormednessReport { violations: vec![] }
    }

    let mut o = obj;
    o = step!(o, RemoveUnneededMuxesPass);
    o = step!(o, RemoveExtraRegistersPass);
    o = step!(o, RemoveUnusedLiterals);
    o = step!(o, RemoveUnusedRegistersPass);
    o = step!(o, PropagateLiteralsPass);
    o = step!(o, DeadCodeEliminationPass);
    o = step!(o, ConstantPropagation);
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_typed_bits_matches_kind_width() {
        let mut rng = seeded_rng(1);
        for &kind in &[Kind::Bits(8), Kind::Bits(16), Kind::Signed(4)] {
            let v = random_typed_bits(kind, &mut rng);
            assert_eq!(v.bits().len(), kind.bits());
            assert_eq!(v.kind(), kind);
        }
    }

    #[test]
    fn random_typed_bits_for_tuple_concatenates_widths() {
        let mut rng = seeded_rng(1);
        let kind = Kind::make_tuple(vec![Kind::Bits(4), Kind::Bits(8), Kind::Bits(2)].into());
        let v = random_typed_bits(kind, &mut rng);
        assert_eq!(v.bits().len(), 4 + 8 + 2);
        assert_eq!(v.kind(), kind);
    }

    #[test]
    fn random_typed_bits_has_no_x_bits() {
        let mut rng = seeded_rng(1);
        let v = random_typed_bits(Kind::Bits(64), &mut rng);
        for b in v.bits() {
            assert!(!matches!(b, crate::bitx::BitX::X), "expected no X bits");
        }
    }

    /// Exercise the per-pass runner against an empty kernel via the
    /// spec test corpus.  We don't have a real kernel handy here,
    /// so this test is more of a smoke test on the API surface.
    /// Real corpus tests live in `crates/rhdl-fpga`.
    #[test]
    fn per_pass_report_data_structure_is_constructible() {
        let r = PerPassReport {
            checkpoints: Vec::new(),
        };
        assert!(r.all_well_formed());
        assert!(r.first_violation().is_none());
    }

    #[test]
    fn lowering_outcome_equality_matches_pattern() {
        assert!(LoweringCorrectnessOutcome::Equal.is_equal());
        assert!(
            !LoweringCorrectnessOutcome::Mismatch {
                rhif_bits: vec![],
                rtl_bits: vec![],
            }
            .is_equal()
        );
    }

    #[test]
    fn semantic_preserved_outcome_matches_pattern() {
        assert!(SemanticPreservationOutcome::Preserved { checkpoints: 5 }.is_preserved());
    }

    /// Suppress "unused — only used by external crates" warnings
    /// for the public functions and types in this module.
    #[allow(dead_code)]
    fn _smoke_test_api_surface() {
        let _: fn(Kind, &mut rand::rngs::StdRng) -> TypedBits = random_typed_bits;
        let _: fn(&Object, &mut rand::rngs::StdRng) -> Vec<TypedBits> = random_arguments;
    }

    // ===========================================================
    // Random RHIF program generator
    // ===========================================================

    /// A freshly-generated random program is well-formed by
    /// construction (we only emit single-assignment, def-before-use
    /// opcodes drawing from a registered slot pool).
    #[test]
    fn random_chain_program_is_well_formed_by_construction() {
        let mut rng = seeded_rng(7);
        for size in [1usize, 2, 4, 8, 16] {
            let obj = generate_chain_program(8, size, &mut rng);
            let report = check_object_universal(&obj);
            assert!(
                report.is_well_formed(),
                "size={size} produced non-well-formed program: {report}",
            );
        }
    }

    /// Run random programs through the subset of stage1 passes
    /// that are kernel-shape-agnostic; verify well-formedness is
    /// preserved at every step.  This is the property the plan
    /// §5.1 calls for: "every pass takes a well-typed Object and
    /// produces a well-typed Object."
    #[test]
    fn random_programs_survive_pass_pipeline() {
        let mut rng = seeded_rng(13);
        for trial in 0..16 {
            let obj = generate_chain_program(8, 6, &mut rng);
            let _ = run_passes_on_random_program(obj).unwrap_or_else(|(pass, r)| {
                panic!("trial #{trial}: pass `{pass}` violated invariants:\n{r}");
            });
        }
    }

    /// Random programs produce the same VM output before and
    /// after the manual pass pipeline runs.  This is the semantic-
    /// preservation property at the synthetic-program level (the
    /// corpus version lives in
    /// `crates/rhdl-fpga/src/widget_property_corpus.rs`).
    ///
    /// Synthesised Objects use [`synthetic_symbol_map`] to satisfy
    /// the VM's preflight `obj.fn_id` lookup, which would
    /// otherwise panic on default-empty SymbolMaps.
    #[test]
    fn random_programs_preserve_semantics_through_passes() {
        let mut rng = seeded_rng(19);
        for trial in 0..16 {
            let obj = generate_chain_program(8, 4, &mut rng);
            let arg_kinds: Vec<Kind> = obj.arguments.iter().map(|r| obj.symtab[*r]).collect();
            let args: Vec<TypedBits> = arg_kinds
                .iter()
                .map(|k| random_typed_bits(*k, &mut rng))
                .collect();
            let pre_result = rhif_execute(&obj, args.clone()).unwrap_or_else(|e| {
                panic!("trial #{trial}: VM error on initial Object: {e:?}");
            });
            let post = run_passes_on_random_program(obj).unwrap_or_else(|(pass, r)| {
                panic!("trial #{trial}: pass `{pass}` violated invariants:\n{r}");
            });
            let post_result = rhif_execute(&post, args).unwrap_or_else(|e| {
                panic!("trial #{trial}: VM error on post-pass Object: {e:?}");
            });
            assert_eq!(
                pre_result.bits(),
                post_result.bits(),
                "trial #{trial}: semantic divergence across pass pipeline",
            );
        }
    }

    // ===========================================================
    // Meta-test (per plan §10)
    // ===========================================================

    /// Each extended generator produces a well-formed program by
    /// construction.  Surfaces well-formedness regressions in any
    /// of the per-shape generator implementations.
    #[test]
    fn extended_generators_produce_well_formed_programs() {
        let mut rng = seeded_rng(31);
        macro_rules! check {
            ($name:expr, $obj:expr) => {{
                let obj: Object = $obj;
                let r = check_object_universal(&obj);
                assert!(r.is_well_formed(), "{} not well-formed:\n{r}", $name,);
            }};
        }
        check!("tuple", generate_tuple_program(8, &mut rng));
        check!("array", generate_array_program(8, 4, &mut rng));
        check!("select", generate_select_program(8, &mut rng));
        check!("repeat", generate_repeat_program(8, 4, &mut rng));
        check!("splice", generate_splice_program(8, &mut rng));
        check!("cast", generate_cast_program(8, 16, 12, &mut rng));
    }

    /// Generators for the remaining 8 RHIF opcodes (`AsBits`,
    /// `AsSigned`, `Retime`, `Wrap` for both `Some` and `None`,
    /// `Struct`, `Case`).  Each is well-formed by construction.
    #[test]
    fn additional_generators_produce_well_formed_programs() {
        let mut rng = seeded_rng(41);
        macro_rules! check {
            ($name:expr, $obj:expr) => {{
                let obj: Object = $obj;
                let r = check_object_universal(&obj);
                assert!(r.is_well_formed(), "{} not well-formed:\n{r}", $name,);
            }};
        }
        check!("as_bits", generate_as_bits_program(8, 16));
        check!("as_signed", generate_as_signed_program(8, 8));
        check!("retime", generate_retime_program(8));
        check!("wrap_some", generate_wrap_some_program(8));
        check!("wrap_none", generate_wrap_none_program(8));
        check!("struct", generate_struct_program(8, &mut rng));
        check!("case", generate_case_program(8, &mut rng));
        check!("enum", generate_enum_program(8));
        check!("exec", generate_exec_program(8, &mut rng));
    }

    /// The additional generators' programs survive the manual
    /// pass pipeline with semantics preserved across passes.
    #[test]
    fn additional_generators_preserve_semantics_through_passes() {
        let mut rng = seeded_rng(43);
        let make_arg_for = |obj: &Object, rng: &mut rand::rngs::StdRng| -> Vec<TypedBits> {
            let kinds: Vec<Kind> = obj.arguments.iter().map(|r| obj.symtab[*r]).collect();
            kinds.iter().map(|k| random_typed_bits(*k, rng)).collect()
        };
        let cases: Vec<(&str, Object)> = vec![
            ("as_bits", generate_as_bits_program(8, 16)),
            ("as_signed", generate_as_signed_program(8, 8)),
            ("retime", generate_retime_program(8)),
            ("wrap_some", generate_wrap_some_program(8)),
            ("wrap_none", generate_wrap_none_program(8)),
            ("struct", generate_struct_program(8, &mut rng)),
            ("case", generate_case_program(8, &mut rng)),
            ("enum", generate_enum_program(8)),
            ("exec", generate_exec_program(8, &mut rng)),
        ];
        for (name, obj) in cases {
            let args = make_arg_for(&obj, &mut rng);
            let pre = match rhif_execute(&obj, args.clone()) {
                Ok(v) => v,
                Err(e) => panic!("{name}: VM error on initial Object: {e:?}"),
            };
            let post = run_passes_on_random_program(obj).unwrap_or_else(|(pass, r)| {
                panic!("{name}: pass `{pass}` violated invariants:\n{r}");
            });
            let post_result = match rhif_execute(&post, args) {
                Ok(v) => v,
                Err(e) => panic!("{name}: VM error on post-pass Object: {e:?}"),
            };
            assert_eq!(
                pre.bits(),
                post_result.bits(),
                "{name}: semantic divergence across pass pipeline",
            );
        }
    }

    /// Each extended generator's program survives the manual pass
    /// pipeline with semantics preserved.  Random argument input;
    /// pre/post VM outcome must agree.
    #[test]
    fn extended_generators_preserve_semantics_through_passes() {
        let mut rng = seeded_rng(37);
        let make_arg_for = |obj: &Object, rng: &mut rand::rngs::StdRng| -> Vec<TypedBits> {
            let kinds: Vec<Kind> = obj.arguments.iter().map(|r| obj.symtab[*r]).collect();
            kinds.iter().map(|k| random_typed_bits(*k, rng)).collect()
        };
        let cases: Vec<(&str, Object)> = vec![
            ("tuple", generate_tuple_program(8, &mut rng)),
            ("array", generate_array_program(8, 4, &mut rng)),
            ("select", generate_select_program(8, &mut rng)),
            ("repeat", generate_repeat_program(8, 4, &mut rng)),
            ("splice", generate_splice_program(8, &mut rng)),
            ("cast", generate_cast_program(8, 16, 12, &mut rng)),
        ];
        for (name, obj) in cases {
            let args = make_arg_for(&obj, &mut rng);
            let pre = match rhif_execute(&obj, args.clone()) {
                Ok(v) => v,
                Err(e) => panic!("{name}: VM error on initial Object: {e:?}"),
            };
            let post = run_passes_on_random_program(obj).unwrap_or_else(|(pass, r)| {
                panic!("{name}: pass `{pass}` violated invariants:\n{r}");
            });
            let post_result = match rhif_execute(&post, args) {
                Ok(v) => v,
                Err(e) => panic!("{name}: VM error on post-pass Object: {e:?}"),
            };
            assert_eq!(
                pre.bits(),
                post_result.bits(),
                "{name}: semantic divergence across pass pipeline",
            );
        }
    }

    /// Plan §10 calls for "a meta-test: deliberately introduce a
    /// known invariant violation in a copy of one pass and verify
    /// the property suite catches it."  This is that test.
    ///
    /// We construct an Object via the random-program generator,
    /// then synthesise a "buggy pass" — one that introduces a
    /// double-assignment violation by re-emitting an existing
    /// opcode's lhs.  The well-formedness checker must catch this.
    /// If the checker does not catch the violation, the property
    /// suite is broken and the corpus tests are vacuous.
    #[test]
    fn meta_test_checker_catches_a_buggy_pass() {
        let mut rng = seeded_rng(23);
        let mut obj = generate_chain_program(8, 4, &mut rng);

        // Confirm the input is well-formed.
        let pre = check_object_universal(&obj);
        assert!(
            pre.is_well_formed(),
            "random program isn't well-formed: {pre}"
        );

        // Buggy pass: duplicate the last opcode (so its lhs is
        // written by both the original and the duplicate).
        let last = obj.ops.last().cloned().expect("at least one op");
        obj.ops.push(last);

        let post = check_object_universal(&obj);
        assert!(
            !post.is_well_formed(),
            "meta-test failed: the buggy pass introduced a double-assignment but the \
             checker did not catch it.  The property suite is vacuous.\n{post}",
        );
        // Specifically, expect a DoubleAssignment violation.
        let has_double = post.violations.iter().any(|v| {
            matches!(
                v,
                crate::rhif::well_formedness::Violation::DoubleAssignment { .. }
            )
        });
        assert!(
            has_double,
            "meta-test failed: violations did not include DoubleAssignment.\n{post}",
        );
    }
}
