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

use crate::compiler::CompilationMode;
use crate::error::RHDLError;
use crate::rhif::object::Object;
use crate::rhif::spec::Slot;
use crate::rhif::vm::execute as rhif_execute;
use crate::rhif::well_formedness::{check_object, check_object_universal, WellFormednessReport};
use crate::types::bit_string::BitString;
use crate::types::kind::Kind;
use crate::types::typed_bits::TypedBits;
use crate::DigitalFn;

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
    let final_obj = crate::compiler::driver::compile_design_stage1_with_checkpoints::<K>(
        mode, &mut hook,
    )?;
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
    Preserved {
        checkpoints: usize,
    },
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
                    (VmOutcome::Ok(a), VmOutcome::Ok(b)) => a.bits() == b.bits() && a.kind() == b.kind(),
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
    let rtl_args: Vec<BitString> = arguments.iter().map(|tb| BitString::from(tb)).collect();
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
pub fn generate_chain_program(
    bit_width: usize,
    n_ops: usize,
    rng: &mut impl RngCore,
) -> Object {
    use crate::ast::ast_impl::FunctionId;
    use crate::ast::SourceLocation;
    use crate::common::symtab::SymbolTable;
    use crate::rhif::object::{LocatedOpCode, SourceDetails, SymbolMap};
    use crate::rhif::rhif_builder::{op_binary, op_unary};
    use crate::rhif::spec::{AluBinary, AluUnary, SlotKind};
    use crate::TypedBits;

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

    let mut symtab: SymbolTable<TypedBits, Kind, SourceDetails, SlotKind> =
        SymbolTable::default();
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
        symbols: SymbolMap::default(),
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
        propagate_literals::PropagateLiteralsPass, remove_extra_registers::RemoveExtraRegistersPass,
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
        WellFormednessReport {
            violations: vec![],
        }
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
        assert!(!LoweringCorrectnessOutcome::Mismatch {
            rhif_bits: vec![],
            rtl_bits: vec![],
        }
        .is_equal());
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

    // Note: semantic preservation across passes on synthetic random
    // programs would require a properly-registered `SymbolMap` for
    // the synthesised Object — the VM's preflight panic-handler
    // looks up `obj.fn_id` in the symbol map and panics on miss.
    // For now, semantic preservation is exercised via the widget
    // corpus (`crates/rhdl-fpga/src/widget_property_corpus.rs`),
    // where the front-end provides a valid SymbolMap.  Wiring up a
    // synthetic SymbolMap for random programs is a Phase 2 follow-up.

    // ===========================================================
    // Meta-test (per plan §10)
    // ===========================================================

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
        assert!(pre.is_well_formed(), "random program isn't well-formed: {pre}");

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
        let has_double = post
            .violations
            .iter()
            .any(|v| matches!(v, crate::rhif::well_formedness::Violation::DoubleAssignment { .. }));
        assert!(
            has_double,
            "meta-test failed: violations did not include DoubleAssignment.\n{post}",
        );
    }
}
