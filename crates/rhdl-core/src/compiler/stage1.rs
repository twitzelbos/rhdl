use log::debug;

use crate::{
    ast::ast_impl::KernelFn,
    compiler::{
        mir::{compiler::compile_mir, infer::infer},
        rhif_passes::{
            check_clock_domain::CheckClockDomain, check_for_rolled_types::CheckForRolledTypesPass,
            check_rhif_flow::DataFlowCheckPass, check_rhif_type::TypeCheckPass,
            constant_propagation::ConstantPropagation,
            dead_code_elimination::DeadCodeEliminationPass,
            lower_dynamic_indices_with_constant_arguments::LowerDynamicIndicesWithConstantArguments,
            lower_inferred_casts::LowerInferredCastsPass,
            lower_inferred_retimes::LowerInferredRetimesPass,
            partial_initialization_check::PartialInitializationCheck, pass::Pass,
            pre_cast_literals::PreCastLiterals,
            precast_integer_literals_in_binops::PrecastIntegerLiteralsInBinops,
            precompute_discriminants::PrecomputeDiscriminantPass,
            propagate_literals::PropagateLiteralsPass, remove_empty_cases::RemoveEmptyCasesPass,
            remove_extra_registers::RemoveExtraRegistersPass,
            remove_unneeded_muxes::RemoveUnneededMuxesPass,
            remove_unused_literals::RemoveUnusedLiterals,
            remove_unused_registers::RemoveUnusedRegistersPass,
            remove_useless_casts::RemoveUselessCastsPass,
            symbol_table_is_complete::SymbolTableIsComplete,
        },
    },
    error::RHDLError,
    rhif::Object,
};

type Result<T> = std::result::Result<T, RHDLError>;

fn wrap_pass<P: Pass>(obj: Object) -> Result<Object> {
    debug!("Running Stage 1 Compiler Pass {}", P::description());
    let obj = P::run(obj)?;
    debug!("Pass complete - checking symbol table");
    let obj = SymbolTableIsComplete::run(obj)?;
    Ok(obj)
}

/// Per-pass observation hook used by [`compile_with_checkpoints`].
///
/// The closure receives `(pass_name, post_pass_object)` after every
/// `wrap_pass` invocation in the compile pipeline.  It is called for
/// every pass in the order they execute (including ones that run in
/// the fixed-point loops), so a single compile may invoke the
/// callback dozens of times.
///
/// This is an intentionally simple-and-flexible signature: returning
/// `Result<()>` lets the observer abort the compile early (e.g., on
/// detecting a property violation), and the `pass_name` is the static
/// `Pass::description()` string, so observers can filter by pass.
pub type CheckpointFn<'a> = dyn FnMut(&'static str, &Object) -> Result<()> + 'a;

fn wrap_pass_observed<P: Pass>(obj: Object, hook: &mut CheckpointFn<'_>) -> Result<Object> {
    let obj = wrap_pass::<P>(obj)?;
    hook(P::description(), &obj)?;
    Ok(obj)
}

/// Like [`compile`], but invokes `hook` after every pass (with the
/// pass's `description` and a reference to the post-pass `Object`).
///
/// Used by Phase 2 of `rhif-formalization-plan.md` to verify that
/// well-formedness and semantic-preservation invariants hold after
/// every pass in the pipeline, not just at the end.  Producing an
/// `Err` from the hook short-circuits the rest of the compile.
///
/// The pass ordering and fixed-point-loop structure exactly mirror
/// [`compile`]; only the checkpoint instrumentation differs.
pub(crate) fn compile_with_checkpoints(
    kernel: &KernelFn,
    mode: CompilationMode,
    hook: &mut CheckpointFn<'_>,
) -> Result<Object> {
    let mir = compile_mir(kernel, mode)?;
    let mut obj = infer(mir)?;
    obj = SymbolTableIsComplete::run(obj)?;
    hook("infer", &obj)?;
    obj = wrap_pass_observed::<CheckForRolledTypesPass>(obj, hook)?;
    let mut hash = obj.hash_value();
    loop {
        obj = wrap_pass_observed::<RemoveUnneededMuxesPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveExtraRegistersPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUnusedLiterals>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUselessCastsPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveEmptyCasesPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUnusedRegistersPass>(obj, hook)?;
        obj = wrap_pass_observed::<PropagateLiteralsPass>(obj, hook)?;
        obj = wrap_pass_observed::<DeadCodeEliminationPass>(obj, hook)?;
        let new_hash = obj.hash_value();
        if new_hash == hash {
            break;
        }
        hash = new_hash;
    }
    if matches!(mode, CompilationMode::Asynchronous) {
        debug!(
            "Running Stage 1 Compiler Pass {}",
            CheckClockDomain::description()
        );
        obj = CheckClockDomain::run(obj)?;
        hook(CheckClockDomain::description(), &obj)?;
    }
    let mut hash = obj.hash_value();
    loop {
        obj = wrap_pass_observed::<PropagateLiteralsPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUnneededMuxesPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveExtraRegistersPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUnusedLiterals>(obj, hook)?;
        obj = wrap_pass_observed::<PreCastLiterals>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUselessCastsPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveEmptyCasesPass>(obj, hook)?;
        obj = wrap_pass_observed::<RemoveUnusedRegistersPass>(obj, hook)?;
        obj = wrap_pass_observed::<DeadCodeEliminationPass>(obj, hook)?;
        obj = wrap_pass_observed::<PrecomputeDiscriminantPass>(obj, hook)?;
        obj = wrap_pass_observed::<LowerInferredCastsPass>(obj, hook)?;
        obj = wrap_pass_observed::<PrecastIntegerLiteralsInBinops>(obj, hook)?;
        obj = wrap_pass_observed::<LowerInferredRetimesPass>(obj, hook)?;
        obj = wrap_pass_observed::<LowerDynamicIndicesWithConstantArguments>(obj, hook)?;
        obj = wrap_pass_observed::<ConstantPropagation>(obj, hook)?;
        let new_hash = obj.hash_value();
        if new_hash == hash {
            break;
        }
        hash = new_hash;
    }
    debug!(
        "Running Stage 1 Compiler Pass {}",
        TypeCheckPass::description()
    );
    obj = TypeCheckPass::run(obj)?;
    hook(TypeCheckPass::description(), &obj)?;
    debug!(
        "Running Stage 1 Compiler Pass {}",
        DataFlowCheckPass::description()
    );
    obj = DataFlowCheckPass::run(obj)?;
    hook(DataFlowCheckPass::description(), &obj)?;
    debug!(
        "Running Stage 1 Compiler Pass {}",
        PartialInitializationCheck::description()
    );
    obj = PartialInitializationCheck::run(obj)?;
    hook(PartialInitializationCheck::description(), &obj)?;
    Ok(obj)
}

#[derive(Debug, Clone, Copy)]
pub enum CompilationMode {
    Asynchronous,
    Synchronous,
}

pub(crate) fn compile(kernel: &KernelFn, mode: CompilationMode) -> Result<Object> {
    let mir = compile_mir(kernel, mode)?;
    let mut obj = infer(mir)?;
    obj = SymbolTableIsComplete::run(obj)?;
    obj = wrap_pass::<CheckForRolledTypesPass>(obj)?;
    let mut hash = obj.hash_value();
    loop {
        obj = wrap_pass::<RemoveUnneededMuxesPass>(obj)?;
        obj = wrap_pass::<RemoveExtraRegistersPass>(obj)?;
        obj = wrap_pass::<RemoveUnusedLiterals>(obj)?;
        obj = wrap_pass::<RemoveUselessCastsPass>(obj)?;
        obj = wrap_pass::<RemoveEmptyCasesPass>(obj)?;
        obj = wrap_pass::<RemoveUnusedRegistersPass>(obj)?;
        obj = wrap_pass::<PropagateLiteralsPass>(obj)?;
        obj = wrap_pass::<DeadCodeEliminationPass>(obj)?;
        let new_hash = obj.hash_value();
        if new_hash == hash {
            break;
        }
        hash = new_hash;
    }
    if matches!(mode, CompilationMode::Asynchronous) {
        debug!(
            "Running Stage 1 Compiler Pass {}",
            CheckClockDomain::description()
        );
        obj = CheckClockDomain::run(obj)?;
    }
    let mut hash = obj.hash_value();
    loop {
        obj = wrap_pass::<PropagateLiteralsPass>(obj)?;
        obj = wrap_pass::<RemoveUnneededMuxesPass>(obj)?;
        obj = wrap_pass::<RemoveExtraRegistersPass>(obj)?;
        obj = wrap_pass::<RemoveUnusedLiterals>(obj)?;
        obj = wrap_pass::<PreCastLiterals>(obj)?;
        obj = wrap_pass::<RemoveUselessCastsPass>(obj)?;
        obj = wrap_pass::<RemoveEmptyCasesPass>(obj)?;
        obj = wrap_pass::<RemoveUnusedRegistersPass>(obj)?;
        obj = wrap_pass::<DeadCodeEliminationPass>(obj)?;
        obj = wrap_pass::<PrecomputeDiscriminantPass>(obj)?;
        obj = wrap_pass::<LowerInferredCastsPass>(obj)?;
        obj = wrap_pass::<PrecastIntegerLiteralsInBinops>(obj)?;
        obj = wrap_pass::<LowerInferredRetimesPass>(obj)?;
        obj = wrap_pass::<LowerDynamicIndicesWithConstantArguments>(obj)?;
        obj = wrap_pass::<ConstantPropagation>(obj)?;
        let new_hash = obj.hash_value();
        if new_hash == hash {
            break;
        }
        hash = new_hash;
    }
    debug!(
        "Running Stage 1 Compiler Pass {}",
        TypeCheckPass::description()
    );
    obj = TypeCheckPass::run(obj)?;
    debug!(
        "Running Stage 1 Compiler Pass {}",
        DataFlowCheckPass::description()
    );
    obj = DataFlowCheckPass::run(obj)?;
    debug!(
        "Running Stage 1 Compiler Pass {}",
        PartialInitializationCheck::description()
    );
    obj = PartialInitializationCheck::run(obj)?;
    Ok(obj)
}
